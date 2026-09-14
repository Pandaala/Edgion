---
name: gateway-link-sys
description: LinkSys external system integration — CRD connector variants, runtime clients, logging DataSenders, and ConfHandler updates.
---

# LinkSys external system integration

> LinkSys is Edgion's unified framework for integrating with external storage / services.
> External system connections are declared via the LinkSys CRD resource; the Gateway manages client lifecycles automatically.

## LinkSys CRD variants

| Provider | Module | Client type | Connection method | Core capabilities |
|----------|--------|-------------|-------------------|-------------------|
| Elasticsearch | `providers/elasticsearch/` | `EsLinkClient` | HTTP bulk API | Bulk indexing, data send |
| Etcd | `providers/etcd/` | `EtcdLinkClient` | gRPC (v3 API) | KV read/write |
| Redis | `providers/redis/` | `RedisLinkClient` | standalone / sentinel / cluster (fred) | Data read/write, distributed rate limiting support |
| Webhook | `providers/webhook/` | WebhookManager | HTTP | External service calls, WebhookKeyGet resolution |
| Kafka | `providers/kafka/` | `KafkaLinkClient` | Kafka producer | Persistent runtime client and log delivery |
| OTLP | `providers/otlp/` | `OtlpLinkClient` | OTLP/gRPC over HTTP/HTTPS | Shared log export, staged replacement, safe egress |
| HttpDns | resource + DNS resolver service | resolver configuration | HTTP DNS | DNS-resolution configuration; not a LinkSys runtime-client store |

`LocalFileWriter` is a logging-only DataSender. OTLP is a dedicated LinkSys
provider; its key-only OtlpLogSender never owns an exporter or probe.

### Elasticsearch

```
providers/elasticsearch/
├── client.rs          # EsLinkClient (connection management, health check)
├── bulk.rs            # Bulk API batch operations
├── log_sender.rs      # key-only DataSender<String> adapter
├── config_mapping.rs  # configuration mapping
├── ops.rs             # ES operation wrappers
└── mod.rs
```

### Redis

```
providers/redis/
├── client.rs          # RedisLinkClient (fred client wrapper)
├── config_mapping.rs  # configuration mapping (standalone / sentinel / cluster)
├── ops.rs             # Redis operation wrappers
└── mod.rs
```

Three deployment modes are supported:
- **Standalone** — single-node Redis.
- **Sentinel** — sentinel-based high availability.
- **Cluster** — Redis Cluster sharding.

### Etcd

> Unlike the other providers, Etcd lives in `edgion-common/src/link_sys_core/providers/etcd/` (re-exported by the gateway), so it can also be used by the controller.

```
edgion-common/src/link_sys_core/providers/etcd/
├── client.rs          # EtcdLinkClient (etcd-client wrapper, background health monitor)
├── config_mapping.rs  # configuration mapping
├── ops.rs             # Etcd operation wrappers
└── mod.rs
```

### Webhook

```
providers/webhook/
├── manager.rs           # ordered prepare/commit publication + call-handle lookup
├── client.rs            # prepared WebhookLinkClient bookkeeping wrapper
├── resolver.rs          # resolve_webhook_key — WebhookKeyGet consumer entry, response extract
├── runtime.rs           # shared sliding-window rate limiter
└── mod.rs
```

> After the **target-unification + request-templating redesign**, the webhook target is
> either a flat Kubernetes Service reference or an origin-only URL. The outbound
> request shape (path / method / allow-listed
> args / headers / cookies, with `origin` / `custom` split and `presence` policy) is owned
> entirely by the webhook resource. `ManagedOutboundClient::from_parts` compiles it during
> ordered LinkSys publication. Consumers pass only `session` + optional `path_override` (no more
> `forwardHeaders` / `pathSuffix`). Full architecture:
> [03-controller-gateway-link/webhook.md](../03-controller-gateway-link/webhook.md).

### LocalFile

```
providers/local_file/
├── data_sender_impl.rs  # DataSender<String> implementation
├── rotation.rs          # log rotation (daily / hourly + expired-file cleanup)
└── mod.rs
```

Characteristics:
- Uses a background thread to write (avoids blocking the tokio runtime).
- `SyncSender<String>` queue with default size `available_cpu_cores * 10,000`.
- `LogType` enum distinguishes log types (Access / Ssl / Tcp / Tls / Udp), used for metric stats.

## LinkSysStore

`LinkSysStore` is the unified management hub for all LinkSys resources:

```rust
pub struct LinkSysStore {
    resources: ArcSwap<LinkSysMap>,  // HashMap<String, LinkSys>
}
```

Operations:
- `replace_all(data)` — full replace (full sync), atomic update + async dispatch to each provider manager.
- `update(add_or_update, remove)` — incremental update (partial sync).
- `get(key)` / `contains(key)` / `count()` — queries.

Global singleton: `get_global_link_sys_store()`.

### Runtime client store

Each provider that requires a persistent connection has its own ArcSwap runtime store:

| Provider | Global store | Accessor |
|----------|--------------|----------|
| Redis | `REDIS_RUNTIME: ArcSwap<HashMap<String, Arc<RedisLinkClient>>>` | `get_redis_client(name)` |
| Etcd | `ETCD_RUNTIME: ArcSwap<HashMap<String, Arc<EtcdLinkClient>>>` | `get_etcd_client(name)` |
| Elasticsearch | `ES_RUNTIME: ArcSwap<HashMap<String, Arc<EsLinkClient>>>` | `get_es_client(name)` |
| Kafka | `KAFKA_RUNTIME` provider store | `get_kafka_client(name)` |
| OTLP | `OTLP_RUNTIME` provider store | `get_otlp_client(name)` |
| Webhook | `WebhookManager` (managed internally) | via `get_webhook_manager()` |

Key format: `"namespace/name"`.

### Client lifecycle

Generic provider processing flow during full_set / partial_update (OTLP and the
ordered Webhook manager have specialized publication boundaries):
1. Dispatch to the corresponding provider based on the `SystemConfig` type.
2. Create a new client instance.
3. Insert into the runtime store first (ensures `get_*_client()` returns the new client immediately).
4. Initialize the connection in the background via `tokio::spawn`.
5. Shut down the old client in the background via `tokio::spawn`.
6. On deletion, attempt removal from every provider's store (because the delete event does not carry type information).

## DataSender

A general-purpose data-sending trait, generic over the payload type:

```rust
#[async_trait]
pub trait DataSender<T>: Send + Sync where T: Send + 'static {
    async fn init(&mut self) -> Result<()>;
    fn healthy(&self) -> bool;
    async fn send(&self, data: T) -> Result<()>;
    async fn send_with_meta(&self, data: T, meta: &LogMeta) -> Result<()>;
    fn name(&self) -> &str;
}
```

- `DataSender<String>` — log delivery (LocalFile, Elasticsearch, Kafka, OTLP). The default
  `send_with_meta` delegates to `send`; structured exporters such as OTLP can use `LogMeta`.
- `LogConfig.fallback` provides one first-healthy standby sink. The supported
  outage-recovery pattern is an Elasticsearch primary plus a bounded LocalFile
  fallback; an operator can replay those files through
  `POST /api/v1/access-log/backfill` after Elasticsearch recovers. There is no
  LinkSys `failedCache` field and no Redis-backed spill cache. Backfill is
  Elasticsearch-only and at-least-once.

## ConfHandler

`ConfHandler<LinkSys>` connects the configuration sync system with LinkSysStore:

```rust
impl ConfHandler<LinkSys> for Arc<LinkSysStore> {
    fn full_set(&self, data: &HashMap<String, LinkSys>) { ... }
    fn partial_update(&self, add, update, remove) { ... }
}
```

- `create_link_sys_handler()` — create a handler and register it with ConfigClient.

## Use cases

- **AccessLog output** — AccessLogger supports LocalFile directly and Elasticsearch,
  Kafka, or OTLP through LinkSys-backed DataSenders.
- **Distributed rate limiting** — the `rate_limit_redis` plugin obtains a Redis client via `get_redis_client()`.
- **External authentication** — the Webhook provider supports WebhookKeyGet resolution (KeyAuth and others then read the written ctx var).
- **Custom integrations** — declare a connection via the LinkSys CRD; business logic obtains the client and uses it.

## Directory layout

After the crate split, the LinkSys **core** (credential / health / lifecycle / TLS) and the **Etcd provider** live in `edgion-common/src/link_sys_core/`; the gateway re-exports them via `edgion-gateway/src/link_sys/mod.rs` (`pub use edgion_common::link_sys_core::{credential, health, lifecycle, tls, LinkSysHealth}`). The gateway crate itself holds the sink/connector providers and the runtime framework. Placement rule: backend-agnostic core logic (or a provider the controller also needs) goes in the common `link_sys_core`; data-plane runtime providers and stores go in `gateway::link_sys`.

```
edgion-gateway/src/link_sys/
├── mod.rs                    # module exports; re-exports core + LinkSysHealth from edgion-common
├── providers/                # gateway-side sink / connector providers
│   ├── elasticsearch/        # ES bulk indexing
│   ├── kafka/                # Kafka async producer
│   ├── otlp/                 # OTLP gRPC log exporter
│   ├── redis/                # Redis multi-mode client
│   ├── webhook/              # HTTP webhook
│   ├── local_file/           # local-file writes + rotation
│   └── mod.rs
└── runtime/                  # runtime framework
    ├── store.rs              # LinkSysStore + per-provider runtime stores
    ├── data_sender.rs        # DataSender trait definition
    ├── conf_handler.rs       # ConfHandler<LinkSys> implementation
    └── mod.rs

edgion-common/src/link_sys_core/       # core framework + Etcd provider (re-exported by the gateway)
├── credential.rs             # materialise credentials from the embedded Secret in LinkSys.spec.*Auth
├── health.rs                 # LinkSysHealth status DTO shared by all providers
├── lifecycle.rs              # unified provider lifecycle (init/shutdown, dispatch)
├── tls.rs                    # unified TLS material resolution for provider connections
├── mod.rs
└── providers/etcd/           # Etcd v3 client (KV / lease / lock + background health monitor)
```

## OTLP generations and teardown

One stable OtlpLinkClient per validated namespace/name owns desired prepared
transport and an ArcSwap active pipeline. A valid update invalidates any older
candidate, prepares immutable auth/TLS/endpoint state, then lets the single
canonical probe connect and complete an empty OTLP Export. Publication checks the same revision under the short
configuration lock. Invalid, unresolved and unreachable replacements keep the
active generation. Full and partial desired-state dispatch run inside the synchronous source-event
critical section so unrelated async task reordering cannot discard an OTLP delta.
Generic full/partial/rebuild tasks also reserve predecessor order inside that
section, then await earlier dispatch completion before entering the Webhook
mutation guard. A foreign-provider-to-OTLP transition therefore removes its old
typed registrations before a later unrelated delta or same-key recreation.
Type change/removal shuts down the owner. The process shutdown fence prevents late
conf-sync events from restarting drained owners. Store stats include otlp_clients.

The probe gives pending preparation and active TCP checking independent 3s budgets
inside an 8s driver deadline. A failed pending connection cannot consume the old
connection's health check. A successful empty Export through the candidate exporter permits publication;
its complete response shares the 3s preparation budget. Native channel readiness
alone can precede mTLS rejection. The same exporter enters the published pipeline,
and failed/stale candidates never start a batch worker. Subsequent health remains
connection-level TCP evidence only.
One resource serves every async log type through the existing scope/record format.

Native tonic014 uses the exact original URI and shared outbound TLS results;
its custom connector resolves/classifies every actual dial. An SDK decorator
bounds the complete export future and polls it with captured runtime context
because the batch processor runs on a std thread. Retained cleanup tasks drain
for 5s, cancel remaining export futures, and await worker join and exporter/channel
release. Only one retiring pipeline is allowed, and cancellation of a shutdown
waiter must not detach cleanup. Read the [OTLP regression boundary](../../04-review/link-sys/regression-guards/otlp-provider-ownership-and-egress.md)
before changing this lifecycle.

Removed keys retain a per-key tombstone until their SDK/probe teardown joins.
Rapid delete/recreate only changes the latest source snapshot during this period;
no second owner starts. Cleanup replays the latest key under the source lock,
unless process shutdown has fenced publication. Retiring stats count tombstones
even while the process drain owns cleanup tasks. A startup guard drains exporters
on errors after resource watches begin, before the startup runtime can drop.
