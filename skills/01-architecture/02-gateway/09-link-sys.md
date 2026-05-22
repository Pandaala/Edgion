---
name: gateway-link-sys
description: LinkSys external system integration — 5 providers, LinkSysStore management, DataSender abstraction, ConfHandler resource updates.
---

# LinkSys external system integration

> LinkSys is Edgion's unified framework for integrating with external storage / services.
> External system connections are declared via the LinkSys CRD resource; the Gateway manages client lifecycles automatically.

## 5 providers

| Provider | Module | Client type | Connection method | Core capabilities |
|----------|--------|-------------|-------------------|-------------------|
| Elasticsearch | `providers/elasticsearch/` | `EsLinkClient` | HTTP bulk API | Bulk indexing, data send |
| Etcd | `providers/etcd/` | `EtcdLinkClient` | gRPC (v3 API) | KV read/write |
| Redis | `providers/redis/` | `RedisLinkClient` | standalone / sentinel / cluster (fred) | Data read/write, distributed rate limiting support |
| Webhook | `providers/webhook/` | WebhookManager | HTTP | External service calls, KeyGet::Webhook resolution |
| LocalFile | `providers/local_file/` | `LocalFileWriter` | Local file I/O | File logging (supports daily / hourly rotation) |

### Elasticsearch

```
providers/elasticsearch/
├── client.rs          # EsLinkClient (connection management, health check)
├── bulk.rs            # Bulk API batch operations
├── data_sender.rs     # DataSender<String> implementation
├── config_mapping.rs  # configuration mapping
├── ops.rs             # ES operation wrappers
└── mod.rs
```

### Redis

```
providers/redis/
├── client.rs          # RedisLinkClient (fred client wrapper)
├── data_sender.rs     # DataSender<String> implementation
├── config_mapping.rs  # configuration mapping (standalone / sentinel / cluster)
├── ops.rs             # Redis operation wrappers
└── mod.rs
```

Three deployment modes are supported:
- **Standalone** — single-node Redis.
- **Sentinel** — sentinel-based high availability.
- **Cluster** — Redis Cluster sharding.

### Etcd

```
providers/etcd/
├── client.rs          # EtcdLinkClient (etcd-client wrapper)
├── config_mapping.rs  # configuration mapping
├── ops.rs             # Etcd operation wrappers
└── mod.rs
```

### Webhook

```
providers/webhook/
├── manager.rs         # WebhookManager (manages webhook instances)
├── runtime.rs         # webhook runtime
├── resolver.rs        # KeyGet::Webhook resolver
├── health.rs          # health check
└── mod.rs
```

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
| Webhook | `WebhookManager` (managed internally) | via `get_webhook_manager()` |

Key format: `"namespace/name"`.

### Client lifecycle

Processing flow during full_set / partial_update:
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
    fn name(&self) -> &str;
}
```

- `DataSender<String>` — text logs (LocalFile, ES, Redis).
- Supports the FailedCache pattern: when ES is unavailable, cache to LocalFile / Redis and replay after recovery.

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

- **AccessLog output** — AccessLogger registers a LocalFileWriter or an ES DataSender.
- **Distributed rate limiting** — the `rate_limit_redis` plugin obtains a Redis client via `get_redis_client()`.
- **External authentication** — the Webhook provider supports KeyGet::Webhook resolution (used by plugins such as key_auth).
- **Custom integrations** — declare a connection via the LinkSys CRD; business logic obtains the client and uses it.

## Directory layout

```
src/core/gateway/link_sys/
├── mod.rs                    # module exports
├── providers/                # implementations of 5 providers
│   ├── elasticsearch/        # ES bulk indexing
│   ├── etcd/                 # Etcd v3 client
│   ├── redis/                # Redis multi-mode client
│   ├── webhook/              # HTTP webhook
│   ├── local_file/           # local-file writes + rotation
│   └── mod.rs
└── runtime/                  # runtime framework
    ├── store.rs              # LinkSysStore + per-provider runtime stores
    ├── data_sender.rs        # DataSender trait definition
    ├── conf_handler.rs       # ConfHandler<LinkSys> implementation
    └── mod.rs
```
