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
| Webhook | `providers/webhook/` | WebhookLinkClient | HTTP | External service calls, WebhookKeyGet resolution |
| Kafka | `providers/kafka/` | `KafkaLinkClient` | Kafka producer | Persistent runtime client and log delivery |
| OTLP | `providers/otlp/` | `OtlpLinkClient` | OTLP/gRPC over HTTP/HTTPS | Persistent runner, shared log export, safe egress |
| HttpDns | Common DNS + typed runtime table | `PreparedHttpDns` | HTTP DNS | Immutable prepared HTTP client or configured fallback-only generation |

`LocalFileWriter` is a logging-only DataSender. OTLP is a dedicated LinkSys
provider; its key-only OtlpLogSender never owns an exporter or probe.

### Elasticsearch

The resources-owned UTC date-pattern validator is shared by Controller LinkSys
validation and `EsCandidate::from_prepared`. Candidate admission runs before
fingerprint/resourceVersion reuse, construction and background task startup.
It tests actual formatting through Chrono's fallible `write_to`, not only parsing.
Default, empty, literal and escaped-percent patterns retain their meaning.

At bulk flush time, `EsBulkConfig::current_index_name` writes directly into the
final String and returns a formatting Result. An unexpected failure drops only
the current owned batch before any ES call or retry, emits a fixed background
diagnostic with the connector key and document count, and lets the worker continue.
Transport health and first-healthy fallback remain unchanged; this is no delivery
acknowledgement or fallback guarantee. Validation runs at configuration time and
index naming remains once per flush, with no new request-path work.

Bulk acknowledgement reception uses the canonical Gateway
`util/http_body.rs` chunk collector. `EsBulkConfig` resolves optional
`bulk.maxResponseBytes` once (default 1 MiB, explicit clamp 1 byte through 8 MiB),
and the effective cap participates in the replacement fingerprint. LinkSys config
interiors remain opaque/preserved in the CRD; the field is serialized to the
Gateway normally. Each HTTP-success receipt is length-prechecked and capped while
streaming, then projected to errors/items/index/status without retaining metadata
or arbitrary error trees. Ignored metadata no longer has incidental required/type
checks. `errors=false` still bypasses per-item correlation. Cap/read/parse failures
use the existing ambiguous no-replay, healthy and retirement-fenced outcome;
429 retries still require exact count and ordered index results. The byte cap
bounds input, not total heap; collector capacity, transport chunks, parser scratch
and projected items also contribute.

```
providers/elasticsearch/
├── apply.rs           # EsApply fingerprint / coverage / build for the runner
├── client.rs          # EsLinkClient (published small handle)
├── bulk.rs            # Bulk API batch operations
├── log_sender.rs      # key-only DataSender<String> adapter
├── config_mapping.rs  # configuration mapping
├── ops.rs             # ES operation wrappers
└── mod.rs
```

### Redis

```
providers/redis/
├── apply.rs           # RedisApply fingerprint / coverage / build for the runner
├── client.rs          # RedisLinkClient (published small handle)
├── config_mapping.rs  # configuration mapping (standalone / sentinel / cluster)
├── ops.rs             # Redis operation wrappers
└── mod.rs
```

Three deployment modes are supported:
- **Standalone** — single-node Redis.
- **Sentinel** — sentinel-based high availability.
- **Cluster** — Redis Cluster sharding.

### Etcd

> Unlike the other providers, Etcd lives in `edgion-common/src/link_sys_core/providers/etcd/` (re-exported by the gateway), so it can also be used by the controller. Gateway apply metadata and per-endpoint coverage sampling are opt-in (`with_gateway_apply_meta`); Controller construction leaves them off (D12).

```
edgion-gateway/src/link_sys/providers/etcd/
├── apply.rs           # EtcdApply fingerprint / coverage / build for the runner
└── mod.rs             # re-exports the shared client + EtcdApply

edgion-common/src/link_sys_core/providers/etcd/
├── client.rs          # EtcdLinkClient (etcd-client wrapper, background health monitor)
├── config_mapping.rs  # configuration mapping
├── ops.rs             # Etcd operation wrappers
└── mod.rs
```

### Webhook

```
providers/webhook/
├── manager.rs           # stateless current-client lookup + call handles
├── client.rs            # provider-owned prepared Webhook client
├── resolver.rs          # resolve_webhook_key — WebhookKeyGet consumer entry, response extract
├── runtime.rs           # shared sliding-window rate limiter
└── mod.rs
```

> After the **target-unification + request-templating redesign**, the webhook target is
> either a flat Kubernetes Service reference or an origin-only URL. The outbound
> request shape (path / method / allow-listed
> args / headers / cookies, with `origin` / `custom` split and `presence` policy) is owned
> entirely by the webhook resource. `ManagedOutboundClient::from_webhook` compiles it during
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
- `replace_all(data)` — full replace (full sync), synchronous runtime preparation
  in source-event order, then publication of the immutable source snapshot.
- `update(add_or_update, remove)` — incremental update (partial sync).
- `get(key)` / `contains(key)` / `count()` — queries.

Global singleton: `get_global_link_sys_store()`.

### Current runtime ownership

One runtime table maps `namespace/name` to a typed provider object. Redis,
Etcd, Elasticsearch, Kafka, and OTLP entries own their existing
`PersistentRunner`; Webhook entries own a prepared `WebhookLinkClient`, and
HttpDns entries own Common's immutable `PreparedHttpDns`.
Typed `get_*_client` APIs select only the matching current entry, then obtain
its published client. Webhook call-handle lookup uses the same table directly.
CredentialSource remains Controller-managed acquisition configuration.

The serialized source snapshot remains separate for configuration consumers.
It is not a second authority for client publication. Counts, keys, health
inspection, and request lookup derive from the unified runtime table.

### Client lifecycle

Ordered full-set, partial-update, and rebuild dispatch share the same mutation
path. The source-event lock spans synchronous runtime preparation/publication
and subsequent raw source publication. Admission retries and the synchronous
OTLP shutdown mutation use that same lock; there is no global dispatch counter
or second registry mutation lock. Cleanup waits happen after releasing it.
Reads remain lock-free. Same-type updates delegate to the provider object.
Persistent runners
retain fingerprint reuse, connection retry/backoff, forced rebuild intent,
generation fencing, and live-connection handover. Webhook prepares a complete
managed outbound client before replacing a same-type entry; equal fingerprints
reuse the current client.

HTTPDNS publishes the latest prepared handle, including its configured fallback
when preparation fails. Equal runtime material reuses the handle; Secret changes
and forced global TLS rebuilds replace it. Queries never build clients or audit
TLS. See [prepared HTTPDNS generations](../../04-review/link-sys/regression-guards/httpdns-prepared-runtime.md).

Deletion removes the current entry and signals its background work to stop.
It does not wait for asynchronous provider shutdown or every caller-held Arc.
A type change is removal followed by creation: temporary unavailability and
brief overlap with stopping old work are accepted. Cleanup never recreates a
key or removes a newer entry. There are no per-provider deleting/pending maps,
tombstones, or cross-type supervisors.

The per-kind capacity limit counts current persistent-provider objects; Webhook
and HTTPDNS are exempt. Desired resources
that cannot yet be admitted remain in the source configuration; removal or cap
increase retries admission from the ordered snapshot. Unresolved credentials
never degrade to anonymous access.

Each persistent client owns a `PublishedHealth` cell. Publication disables the
old reporter and establishes the new generation's baseline; retired observations
do not increment health-transition counters. Gateway clients inject the
binary-owned reporter, while Controller-created Etcd clients do not.

Webhook removal/replacement explicitly stops its managed health task even if
an older request retains a call handle. Calls already holding a client are not
subject to a new blanket revocation policy. Retirement does not promise delivery
of queued records.


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

Process-output `LinkSysRef` values require explicit DNS-valid namespace/name and
use the canonical reference identity check before producing a runtime key.
ES, Kafka, and OTLP sender constructors reject invalid programmatic references;
async factories log only the checked identity. This process configuration has no
Kubernetes source owner, namespace default, or ReferenceGrant. Sender health and
send operations retain live typed provider lookup, including replacement/deletion.
Access-log backfill captures no ES key for an invalid programmatic reference;
valid disabled configurations and fallback path capture retain their lifecycle.

## ConfHandler

`ConfHandler<LinkSys>` connects the configuration sync system with LinkSysStore:

```rust
impl ConfHandler<LinkSys> for LinkSysStore {
    fn full_set(&self, data: &HashMap<String, Arc<LinkSys>>) { ... }
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

After the crate split, the LinkSys **core** (credential / health / TLS) and the **Etcd provider** live in `edgion-common/src/link_sys_core/`; the gateway re-exports them via `edgion-gateway/src/link_sys/mod.rs` (`pub use edgion_common::link_sys_core::{credential, health, tls, LinkSysHealth}`). The gateway crate itself holds the sink/connector providers and the runtime framework. Placement rule: backend-agnostic core logic (or a provider the controller also needs) goes in the common `link_sys_core`; data-plane runtime providers and stores go in `gateway::link_sys`.

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
    ├── store.rs              # LinkSysStore + unified typed runtime table
    ├── data_sender.rs        # DataSender trait definition
    ├── conf_handler.rs       # ConfHandler<LinkSys> implementation
    └── mod.rs

edgion-common/src/link_sys_core/       # core framework + Etcd provider (re-exported by the gateway)
├── credential.rs             # materialise credentials from the embedded Secret in LinkSys.spec.*Auth
├── health.rs                 # Shared health DTO and publication-aware health cell
├── tls.rs                    # unified TLS material resolution for provider connections
├── mod.rs
└── providers/etcd/           # Etcd v3 client (KV / lease / lock + background health monitor)
```

## OTLP generations and teardown

OTLP uses the shared per-key `PersistentRunner`, the same lifecycle shell as
Redis, Etcd, Elasticsearch, and Kafka. Each `OtlpLinkClient` is one immutable
generation containing prepared transport, fingerprint, exporter pipeline, probe,
and health state. The runner builds and retries a candidate until its bounded
empty OTLP Export succeeds, then publishes that same exporter. A stale candidate
whose desired fingerprint changed is shut down without publication. Equal
fingerprints reuse the published generation; changed healthy generations use
handover, while a generation with zero live coverage is replaced immediately.

Full, partial, and rebuild dispatch post OTLP desired state from the same ordered
generic task as the other persistent providers. Kind changes delete foreign
runners before applying OTLP. Removal immediately unpublishes and signals runner
cleanup without waiting for it. Unresolved credentials or TLS material do not
create a runner. No deferred recreation state exists. Store list/count reports
current runner keys, including connecting unpublished keys, while `get_otlp_client`
returns `None` until first publication.

A successful empty Export through the candidate exporter permits publication;
its complete response shares the 3s preparation budget. Native channel readiness
alone can precede mTLS rejection. Subsequent health remains connection-level TCP
evidence only.
One resource serves every async log type through the existing scope/record format.

Native tonic014 uses the exact original URI and shared outbound TLS results;
its custom connector resolves/classifies every actual dial. An SDK decorator
bounds the complete export future and polls it with captured runtime context
because the batch processor runs on a std thread. Retained cleanup tasks drain
for 5s, cancel remaining export futures, and await worker join and exporter/channel
release. The runner waits 10 seconds before retiring a handed-over generation and
joins retirement during deletion. Read the [OTLP regression boundary](../../04-review/link-sys/regression-guards/otlp-provider-ownership-and-egress.md)
before changing this lifecycle.

Removed entries signal runner cancellation without blocking admission of a new
same-key entry. OTLP retains its provider-specific process-exit cleanup so the SDK
can retire while Tokio IO remains available, including startup failure after
resource watches begin. This is not a cross-provider deletion barrier or a
lossless-log-delivery guarantee.
