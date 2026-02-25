# Edgion Project Architecture

> Comprehensive architecture reference for the Edgion API Gateway system. Single-crate Rust project with three binaries: controller (control plane), gateway (data plane), and edgion-ctl (CLI tool).
>
> **TODO (2026-02-25): Small Improvement**
> - [ ] Add CI/CD and build pipeline overview section (`.github/workflows/ci.yml`, `build-image.yml`, `docker/Dockerfile`, cargo-chef multi-stage build)
> (Note: Annotation system and Feature flags split into separate docs)
## High-Level Architecture

```
                    ┌──────────────────────────────────────────────────────────┐
                    │                  edgion-controller                       │
                    │                                                          │
  YAML/K8s CRD ──► │  ConfCenter ──► Workqueue ──► ResourceProcessor          │
                    │  (File/K8s)     (per-kind)    (validate/preparse/parse)  │
                    │                                                          │
  edgion-ctl ────► │  Admin API (:5800)   ConfigSyncServer (gRPC :5810)       │
                    └─────────────────────────────┬────────────────────────────┘
                                                  │ gRPC Watch/List
                                                  ▼
                    ┌──────────────────────────────────────────────────────────┐
                    │                  edgion-gateway                          │
                    │                                                          │
                    │  ConfigSyncClient ──► ClientCache ──► Preparse           │
                    │                       (per-kind)                         │
                    │  Pingora Server                                          │
                    │  ├─ ConnectionFilter (TCP-level, StreamPlugins)          │
                    │  ├─ ProxyHttp (HTTP/gRPC lifecycle)                      │
                    │  │  ├─ request_filter     → route match + plugins        │
                    │  │  ├─ upstream_peer      → backend selection + LB       │
                    │  │  ├─ upstream_response  → response plugins             │
                    │  │  └─ logging            → AccessLog                    │
                    │  └─ TCP/UDP/TLS Routes                                   │
                    │                                                          │
                    │  Admin API (:5900)   Metrics API (:5901)                 │
                    └──────────────────────────────────────────────────────────┘
```

## Crate Structure

Single crate (not a workspace), three `[[bin]]` targets:

| Binary | Path | Runtime | Role |
|--------|------|---------|------|
| `edgion-gateway` | `src/bin/edgion_gateway.rs` | Sync (Tokio created internally, Pingora main loop) | Data plane |
| `edgion-controller` | `src/bin/edgion_controller.rs` | `#[tokio::main(multi_thread)]` | Control plane |
| `edgion-ctl` | `src/bin/edgion_ctl.rs` | `#[tokio::main]` | CLI tool |

Example binaries for testing: `test_server`, `test_client`, `test_client_direct`, `resource_diff`, `config_load_validator`.

Default features: `allocator-jemalloc` + `boringssl`.

## Code Architecture: bin / core / types

```
src/
├── bin/                         # Binary entry points (thin wrappers)
│   ├── edgion_gateway.rs        #   → EdgionGatewayCli::run()
│   ├── edgion_controller.rs     #   → EdgionControllerCli::run()
│   └── edgion_ctl.rs            #   → Cli::run()
├── lib.rs                       # Crate root: pub mod core, pub mod types
├── core/                        # All business logic
│   ├── cli/                     # CLI parsing + startup wiring
│   ├── conf_mgr/                # Config management (controller core)
│   ├── conf_sync/               # gRPC sync (server + client + cache)
│   ├── api/                     # HTTP APIs (controller admin, gateway admin, metrics)
│   ├── gateway/                 # Gateway config, listeners, route dispatch
│   ├── routes/                  # HTTP, gRPC, TCP, TLS, UDP route processing
│   ├── plugins/                 # Plugin system (edgion_plugins, stream_plugins, gapi_filters)
│   ├── backends/                # Backend discovery (Service, EndpointSlice, Endpoint)
│   ├── lb/                      # Load balancing (EWMA, LeastConn, WeightedSelector)
│   ├── tls/                     # TLS termination, cert management
│   ├── observe/                 # Logging: access_log, ssl_log, tcp_log, udp_log, metrics
│   ├── link_sys/                # External system connectors (file, ES, Kafka, Redis)
│   ├── matcher/                 # Host matching, IP radix tree
│   ├── services/                # ACME certificate automation
│   └── utils/                   # Duration parsing, metadata filter, networking, real IP
└── types/                       # Shared type definitions
    ├── resource/                # Resource system (define_resources!, ResourceKind, ResourceMeta)
    ├── resources/               # Per-kind resource structs (Gateway, HTTPRoute, EdgionPlugins, ...)
    ├── common/                  # KeyGet/KeySet unified accessors
    ├── constants/               # Annotations, labels, headers, secret keys
    ├── ctx.rs                   # EdgionHttpContext (per-request state)
    ├── filters.rs               # PluginRunningResult, PluginRunningStage, PluginTags
    ├── schema.rs                # JSON schema validation
    └── err.rs                   # Error types
```

**Design principle:** `types/` is pure data definitions (no business logic), `core/` is all logic. Binaries in `bin/` are thin wrappers that parse CLI and call into `core/cli/`.

---

## Controller Architecture

### ConfCenter — Multi Config Center Support

The controller abstracts its config source behind the `ConfCenter` trait:

```
ConfMgr (facade, in manager.rs)
└── Arc<dyn ConfCenter>
    ├── FileSystemCenter   — watches local YAML directory, file events
    └── KubernetesCenter   — K8s API watchers, leader election
```

**Traits:**
- `CenterApi` — CRUD: `set_one`, `create_one`, `update_one`, `delete_one`, `get_one`, `list_all`
- `CenterLifeCycle` — `start`, `is_ready`, `config_sync_server`, `request_reload`
- `ConfCenter = CenterApi + CenterLifeCycle`

**Key files:**
- `src/core/conf_mgr/conf_center/traits.rs` — trait definitions
- `src/core/conf_mgr/conf_center/file_system/center.rs` — `FileSystemCenter`
- `src/core/conf_mgr/conf_center/kubernetes/center.rs` — `KubernetesCenter`
- `src/core/conf_mgr/manager.rs` — `ConfMgr` facade

### Workqueue — Per-Resource Processing

Each resource kind gets its own `Workqueue` + `ResourceProcessor`:

```
Event (file change / K8s watch)
  → ResourceController.on_apply(key) / on_delete(key)
    → Workqueue.enqueue(key)        # Deduplicated by pending set
      → Worker loop:
        item = dequeue()            # Key released from pending (allows dirty requeue)
        obj = store.get(key)
        handler.validate(obj)       # Schema + semantic validation
        handler.preparse(obj)       # Build runtime structures
        handler.parse(obj)          # Update caches, resolve refs
        handler.on_change(obj)      # Notify dependents
        handler.update_status(obj)  # Write status back
```

**Requeue with backoff:** `initial_backoff * 2^retry_count`, capped by `max_backoff`. Items dropped after `max_retries`.

**Dirty requeue:** Key is removed from `pending` on dequeue, so new events for the same key can be enqueued while processing. This ensures no events are lost.

**Key files:**
- `src/core/conf_mgr/sync_runtime/workqueue.rs` — `Workqueue`, `WorkItem`, `WorkqueueConfig`
- `src/core/conf_mgr/sync_runtime/resource_processor/processor.rs` — `ResourceProcessor<K>`
- `src/core/conf_mgr/sync_runtime/resource_processor/handler.rs` — `ProcessorHandler` trait
- `src/core/conf_mgr/sync_runtime/resource_processor/handlers/` — per-kind handlers
- `src/core/conf_mgr/processor_registry.rs` — `PROCESSOR_REGISTRY` (global, for cross-kind requeue)

### Cross-Resource Requeue

When one resource changes, dependent resources are requeued automatically.

**Secret → dependent resources:**

```
SecretHandler.on_change()
  → SecretRefManager.get_refs(secret_key)     # Returns Set<ResourceRef>
    → for each ref: PROCESSOR_REGISTRY.requeue(kind, key)
      → target kind's workqueue.enqueue(key)
```

`SecretRefManager` maintains bidirectional mappings:
- Forward: `secret_key → Set<ResourceRef>` (which resources depend on this secret)
- Reverse: `resource_key → Set<secret_key>` (which secrets this resource uses)

Handlers register refs: `ctx.secret_ref_manager().add_ref(secret_key, resource_ref)`

**ReferenceGrant → cross-namespace resources:**

```
ReferenceGrant change
  → CrossNsRevalidationListener
    → requeue all resources with cross-namespace refs
      (HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, UDPRoute)
```

**Key files:**
- `src/core/conf_mgr/sync_runtime/resource_processor/secret_utils/secret_ref.rs` — `SecretRefManager`
- `src/core/conf_mgr/sync_runtime/resource_processor/secret_utils/secret_store.rs` — `GLOBAL_SECRET_STORE`
- `src/core/conf_mgr/sync_runtime/resource_processor/ref_grant/` — `CrossNamespaceRefManager`, revalidation

### Secret — Built-in Mechanism

```
GLOBAL_SECRET_STORE (LazyLock<SecretStore>)
├── Map: "namespace/name" → Secret
├── get_secret(namespace, name) → Option<Secret>
├── update_secrets(upsert, remove)
└── replace_all_secrets()

SecretHandler
├── parse: updates SecretStore
├── on_change: triggers cascading requeue for dependents
└── on_delete: removes from store + triggers requeue
```

Plugins access secrets at runtime: `get_secret(Some(namespace), &secret_ref.name)`.

Controller-side handlers resolve secrets during parse phase and populate `resolved_*` fields in configs (e.g., `resolved_client_secret` in plugin configs).

### Controller Admin API

HTTP on `:5800` via Axum:

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/health` | Liveness |
| GET | `/ready` | Readiness (ConfigServer ready) |
| GET | `/api/v1/server-info` | Server ID, endpoint mode, supported kinds |
| POST | `/api/v1/reload` | Reload all resources from storage |
| GET/POST/PUT/DELETE | `/api/v1/namespaced/{kind}/{namespace}[/{name}]` | Namespaced resource CRUD |
| GET/POST/PUT/DELETE | `/api/v1/cluster/{kind}[/{name}]` | Cluster-scoped resource CRUD |
| GET | `/configserver/{kind}/list` | List from ConfigServer cache |

---

## Gateway Architecture

### Startup Sequence

```
1. Load config (EdgionGatewayConfig)
2. Create ConfigSyncClient → connect to controller gRPC
3. Fetch server info (endpoint mode, supported kinds)
4. Start watching all resource kinds from controller
5. Start auxiliary services (backend cleaner, admin API :5900, metrics :5901)
6. Wait until all caches ready
7. Preload load balancers
8. Initialize loggers (access, SSL, TCP, UDP)
9. Configure Pingora listeners via GatewayBase
10. Run Pingora server (blocks until shutdown)
```

### Pingora ProxyHttp — HTTP/gRPC Lifecycle

`EdgionHttp` implements `pingora_proxy::ProxyHttp` with `CTX = EdgionHttpContext`:

```
Client Request
  │
  ▼
early_request_filter()     ← ACME HTTP-01 challenge handling
  │
  ▼
request_filter()           ← Core: metadata extraction, route matching,
  │                          plugin chain (RequestFilter), XFF/X-Real-IP
  │                          Sets ctx.plugin_running_result
  ▼
upstream_peer()            ← Backend selection (HTTP vs gRPC), LB, timeout config
  │                          Checks plugin_running_result for early termination
  ▼
connected_to_upstream()    ← Connection established callback
  │
  ▼
upstream_response_filter() ← Sync: response plugins (UpstreamResponseFilter),
  │                          server header, status/timing recording
  ▼
upstream_response_body_filter() ← Sync per-chunk: bandwidth limiting
  │
  ▼
response_filter()          ← Async response processing
  │
  ▼
logging()                  ← Metrics update, AccessLogEntry build + send
```

**Key files:** `src/core/routes/http_routes/proxy_http/pg_*.rs` (one file per hook)

### Connection Filter — TCP-Level (StreamPlugins)

Runs before TLS/HTTP, at raw TCP level:

```
TCP Connection arrives
  → ConnectionFilter.check(session)
    → StreamPluginConnectionFilter
      → StreamPluginStore.get(store_key)
      → StreamPluginRuntime.run(&StreamContext)
        → Each plugin: Allow or Deny(reason)
      → First Deny wins → reject connection
```

Configured per Gateway listener via annotation: `edgion.io/edgion-stream-plugins: "namespace/name"`.

**Key files:**
- `src/core/plugins/edgion_stream_plugins/connection_filter_bridge.rs`
- `src/core/plugins/edgion_stream_plugins/stream_plugin_runtime.rs`
- `src/core/gateway/listener_builder.rs` — `apply_connection_filter()`

### Plugin System

Four plugin stages, each with its own trait:

| Trait | Timing | Async | Signature |
|-------|--------|-------|-----------|
| `RequestFilter` | Before upstream | Yes | `run_request(&self, session, log) → PluginRunningResult` |
| `UpstreamResponseFilter` | After upstream headers | No | `run_upstream_response_filter(&self, session, log) → PluginRunningResult` |
| `UpstreamResponseBodyFilter` | Per body chunk | No | `run_upstream_response_body_filter(&self, body, eos, session, log) → Option<Duration>` |
| `UpstreamResponse` | After upstream (full) | Yes | `run_upstream_response(&self, session, log) → PluginRunningResult` |

**Plugin chain execution (`PluginRuntime`):**

```rust
// run_request_plugins: runs all RequestFilter plugins in order
for plugin in &self.request_filters {
    let result = plugin.run_request(session, log).await;
    match result {
        GoodNext | Nothing => continue,
        ErrTerminateRequest => { ctx.plugin_running_result = ErrTerminateRequest; break; }
        ErrResponse { .. } => { ctx.plugin_running_result = result; break; }
    }
}
```

**Conditional wrapping:** All plugins are automatically wrapped in `ConditionalRequestFilter` / `ConditionalUpstreamResponseFilter` which evaluates skip/run conditions before executing the plugin.

**Plugin preparse:** `PluginRuntime` is built during HTTPRoute/GRPCRoute preparse (not at request time), stored on the route rule. This means plugin instantiation happens once per config change, not per request.

**Key files:**
- `src/core/plugins/plugin_runtime/runtime.rs` — `PluginRuntime`
- `src/core/plugins/plugin_runtime/conditional_filter.rs` — condition wrapping
- `src/core/plugins/plugin_runtime/traits/` — all trait definitions
- `src/core/plugins/edgion_plugins/` — plugin implementations

### Access Log — High Efficiency Design

Goal: **one access log line captures all behavior/errors for a request**.

```
EdgionHttpContext (per-request, carried through entire lifecycle)
  │
  │  Contains:
  │  ├── request_info (client_addr, path, hostname, trace_id, ...)
  │  ├── edgion_status (error codes, warnings)
  │  ├── backend_context (service, upstream attempts, connect time)
  │  ├── stage_logs (Vec<StageLogs>: plugin execution logs per stage)
  │  ├── plugin_running_result (final plugin result)
  │  └── ctx_map (plugin-set variables)
  │
  ▼  At logging() hook:
AccessLogEntry::from_context(ctx)    ← Borrows from ctx, zero copy
  │
  ▼
entry.to_json()                      ← Single serde_json::to_string()
  │
  ▼
access_logger.send(json).await       ← Async, non-blocking
  │
  ▼
DataSender<String>                   ← Pluggable output via LinkSys
  ├── LocalFileWriter (default)        (queue + rotation)
  ├── Elasticsearch (future)
  └── Kafka (future)
```

**PluginLog budget:** Fixed 100-byte `SmallVec` buffer per plugin, stack-allocated (zero heap). Overflow tracked by `log_full` flag. Each plugin writes concise outcome strings: `"OK u=jack; "`, `"Deny ip=1.2.3.4; "`.

**Key files:**
- `src/types/ctx.rs` — `EdgionHttpContext`
- `src/core/observe/access_log/entry.rs` — `AccessLogEntry`
- `src/core/observe/access_log/logger.rs` — `AccessLogger`
- `src/core/observe/access_log/logger_factory.rs` — `create_async_logger()`
- `src/core/plugins/plugin_runtime/log.rs` — `PluginLog`, `LogBuffer` (100-byte SmallVec)

### LinkSys Design

LinkSys is a CRD for declaring external system connections:

```yaml
apiVersion: edgion.io/v1alpha1
kind: LinkSys
spec:
  system:
    redis:
      endpoints: [...]
    # or: etcd, elasticsearch, kafka
```

**`SystemConfig` variants:** `Redis`, `Etcd`, `Elasticsearch`, `Kafka`

**Core abstraction:** `DataSender<T>` trait — async send to any backend. Currently implemented:
- `LocalFileWriter` — file output with rotation (for access/TCP/UDP/SSL logs)
- Future: ES, Kafka via LinkSys config

**Usage:** Observability sinks (access log, TCP log, UDP log, SSL log), rate limit state (future: Redis-backed).

**Key files:**
- `src/types/resources/link_sys/` — CRD type definitions
- `src/core/link_sys/` — `DataSender`, `LocalFileWriter`
- `src/types/output.rs` — `StringOutput` (local file vs external)

---

## gRPC Communication — Controller ↔ Gateway

### Proto Definition

`src/core/conf_sync/proto/config_sync.proto`:

```protobuf
service ConfigSync {
    rpc GetServerInfo(ServerInfoRequest) returns (ServerInfoResponse);
    rpc List(ListRequest) returns (ListResponse);
    rpc Watch(WatchRequest) returns (stream WatchResponse);
    rpc WatchServerMeta(WatchServerMetaRequest) returns (stream ServerMetaEvent);
}
```

### Sync Flow

```
Gateway startup:
  1. GetServerInfo() → server_id, endpoint_mode, supported_kinds
  2. For each kind: List(kind) → full snapshot
  3. For each kind: Watch(kind, from_version) → streaming updates

Controller reload:
  1. Controller generates new server_id
  2. Watch stream sends WATCH_ERR_SERVER_RELOAD
  3. Gateway detects server_id change
  4. Gateway re-Lists all kinds (full re-sync)
```

### Server Side (Controller)

```
PROCESSOR_REGISTRY
  → all_watch_objs(no_sync_kinds)     # Builds WatchObj per kind
    → ConfigSyncServer { watch_objs }
      → ConfigSyncGrpcServer serves List/Watch
        → ConfigSyncServerProvider for reload (swap server on reload)
```

`ReferenceGrant` and `Secret` are `no_sync_kinds` — not sent to Gateway.

### Client Side (Gateway)

```
ConfigSyncClient
  → per-kind ClientCache<T>
    → Watch stream → ConfHandler { full_set, partial_update }
      → cache_data updated (ArcSwap for lock-free reads)
      → preparse triggered on update
```

**Key files:**
- `src/core/conf_sync/proto/config_sync.proto` — proto definition
- `src/core/conf_sync/conf_server/` — gRPC server, `ConfigSyncServer`
- `src/core/conf_sync/conf_client/grpc_client.rs` — `ConfigSyncClient`
- `src/core/conf_sync/cache_client/cache.rs` — `ClientCache<T>`, `DynClientCache`

---

## Resource System

### Single Source of Truth — `define_resources!`

All resources are declared once in `src/types/resource/defs.rs` via the `define_resources!` macro:

```rust
define_resources! {
    Gateway => {
        kind_name: "Gateway",
        kind_aliases: &["gw"],
        cache_field: gateway_cache,
        capacity_field: gateway_capacity,
        default_capacity: 10,
        cluster_scoped: false,
        is_base_conf: false,
        in_registry: true,
    },
    // ... all other kinds
}
```

This generates: `ResourceKind` enum, `from_kind_name()`, `from_content()`, registry metadata.

### ResourceMeta Trait

Every resource implements `ResourceMeta` (via `impl_resource_meta!`):

```rust
pub trait ResourceMeta {
    fn get_version(&self) -> u64;
    fn resource_kind() -> ResourceKind;
    fn kind_name() -> &'static str;
    fn key_name(&self) -> String;           // "namespace/name"
    fn pre_parse(&mut self) { }             // Optional preparse hook
}
```

### ResourceKind Enum

`GatewayClass`, `EdgionGatewayConfig`, `Gateway`, `HTTPRoute`, `GRPCRoute`, `TCPRoute`, `TLSRoute`, `UDPRoute`, `Service`, `EndpointSlice`, `Endpoint`, `Secret`, `EdgionTls`, `EdgionPlugins`, `EdgionStreamPlugins`, `PluginMetaData`, `LinkSys`, `ReferenceGrant`, `BackendTLSPolicy`, `EdgionAcme`

### Resource Preparse

Preparse builds runtime-ready structures at config-load time (not per-request):

| Resource | Preparse Purpose |
|----------|-----------------|
| `HTTPRoute` | Build `PluginRuntime` from filters, parse timeouts, resolve `ExtensionRef` LB |
| `GRPCRoute` | Same as HTTPRoute (hidden logic, timeouts) |
| `EdgionPlugins` | Validate all plugin configs, fill `preparse_errors` |
| `LinkSys` | Validate endpoints, topology |
| `EdgionTls` | Validate TLS config |

Preparse runs in **both** controller (for status reporting) and gateway (for runtime structures).

---

## EdgionHttpContext (Per-Request State)

`src/types/ctx.rs` — the "carry bag" through the entire HTTP request lifecycle:

| Field | Purpose |
|-------|---------|
| `start_time` | Request timing |
| `gateway_info` | Gateway metadata |
| `request_info` | Client addr, remote addr, hostname, path, trace ID, SNI, gRPC metadata |
| `edgion_status` | Error codes accumulated during processing |
| `route_unit` / `grpc_route_unit` | Matched route rule (contains `PluginRuntime`) |
| `selected_backend` / `selected_grpc_backend` | Chosen backend ref |
| `backend_context` | Service name, upstream attempts, connect time |
| `stage_logs` | `Vec<StageLogs>` — plugin logs per execution stage |
| `pending_edgion_plugins_logs` | For nested ExtensionRef plugin execution |
| `plugin_ref_stack` | Cycle detection for nested plugin refs |
| `plugin_running_result` | Current plugin chain result |
| `ctx_map` | `HashMap<String, String>` — plugin-set variables |
| `path_params` | Lazy-extracted route path parameters |
| `hash_key` | Consistent hashing key |
| `try_cnt` | Upstream connection attempt counter |

Created in `new_ctx()`, consumed in `logging()`. Plugins interact via `PluginSession` adapter.

---

## edgion-ctl CLI

```
edgion-ctl [--server URL] [--socket PATH] [--target center|server|client] <COMMAND>
```

| Command | Target | Description |
|---------|--------|-------------|
| `apply -f <file/dir>` | center | Apply YAML resources (create or update) |
| `get <kind> [name] -n <ns>` | all | Get resources (table/json/yaml/wide output) |
| `delete <kind> <name> -n <ns>` | center | Delete a resource |
| `delete -f <file>` | center | Delete resources from file |
| `reload` | center | Reload all resources from storage |

**Target types:**
- `center` (default) — ConfCenter API on controller (:5800), supports CRUD
- `server` — ConfigServer cache on controller (:5800), read-only
- `client` — ConfigClient cache on gateway (:5900), read-only

Useful for debugging: compare `server` vs `client` to check sync status.

---

## Testing Infrastructure

| Component | Path | Purpose |
|-----------|------|---------|
| `test_server` | `examples/test/code/server/test_server.rs` | Multi-protocol echo backend (HTTP, gRPC, WebSocket, TCP, UDP, auth) |
| `test_client` | `examples/test/code/client/test_client.rs` | Suite-based test runner with `TestSuite` trait |
| `resource_diff` | `examples/test/code/validator/resource_diff.rs` | Controller ↔ Gateway sync verification |
| `run_integration.sh` | `examples/test/scripts/integration/` | Full integration test orchestrator |
| Test configs | `examples/test/conf/` | YAML resources organized by `Resource/Item/` |
| Port registry | `examples/test/conf/ports.json` | Unique port allocation per test suite |

See `docs/skills/integration-testing.md` for detailed integration testing guide.

---

## Key Dependencies

| Category | Crates | Purpose |
|----------|--------|---------|
| **Proxy core** | `pingora-core`, `pingora-proxy`, `pingora-http`, `pingora-load-balancing` | HTTP proxy engine |
| **Async** | `tokio`, `tokio-stream`, `futures`, `async-trait` | Async runtime |
| **gRPC** | `tonic`, `tonic-reflection`, `prost` | Controller ↔ Gateway communication |
| **HTTP API** | `axum`, `tower-http`, `hyper-util` | Admin APIs |
| **K8s** | `kube`, `k8s-openapi`, `schemars` | K8s integration + CRD schema |
| **Serialization** | `serde`, `serde_json`, `serde_yaml`, `toml` | Config parsing |
| **TLS** | `rustls`, `tokio-rustls`, `boring-sys` | TLS termination (rustls or BoringSSL) |
| **Observability** | `tracing`, `metrics` | Logging + metrics |
| **Security** | `jsonwebtoken`, `bcrypt`, `base64` | Auth plugins |
| **Networking** | `reqwest` | Plugin HTTP client (external calls) |
| **Performance** | `tikv-jemallocator`, `dashmap`, `arc-swap`, `smallvec` | Memory allocator, concurrent maps, lock-free reads, stack buffers |
