---
name: link-controller-side
description: Controller-side gRPC implementation — ConfigSyncServer, ConfigSyncGrpcServer, ConfigSyncServerProvider, WatchObj trait, ClientRegistry.
---

# Controller-side implementation

The Controller exposes the gRPC config sync service via `ConfigSyncServer` + `ConfigSyncGrpcServer`. ResourceProcessor writes processed resources into `ServerCache<T>`; ServerCache implements the `WatchObj` trait to provide a uniform list/watch interface; ConfigSyncServer aggregates all WatchObjs and exposes them to the Gateway over gRPC.

## ConfigSyncServer

The core state manager of the gRPC config sync server. It holds the WatchObj for every resource and provides a unified list/watch API.

```rust
pub struct ConfigSyncServer {
    server_id: RwLock<String>,                              // Server instance ID (millisecond timestamp)
    watch_objects: RwLock<HashMap<String, Arc<dyn WatchObj>>>, // WatchObjs registered by kind
    client_registry: Arc<ClientRegistry>,                    // Registry of connected Gateway instances
}
```

### server_id

A process-monotonic millisecond string generated when a new `ConfigSyncServer` is constructed.
ConfCenter reload/relink publishes a new server instance rather than mutating an existing ID.

### WatchObj registration

Processors register in the runtime-owned `ProcessorRegistry`. After every expected processor is
registered and every sync kind is ready, the ConfCenter collects
`processor_registry.all_watch_objs(no_sync_kinds)`, calls production `register_all`, and publishes
the complete server. Per-kind `register_watch_obj` exists only for tests.

### list/watch operations

| Operation | Flow |
|------|------|
| `list(kind)` | Look up WatchObj → `list_json()` → return `ListDataSimple { data, sync_version, server_id }`. Uncached; used only by the Admin `/configserver` path (redaction isolation). |
| `list_sync_cached(kind)` | Look up WatchObj → per-(kind, sync_version) serialized-bytes cache with single-flight fill → `list_json()` only on a version miss → return `ListDataSimple`. Used by the gRPC `List` RPC and the federation client — both real-secret transports; serialization happens at most once per (kind, sync_version). |
| `watch(kind, client_id, ...)` | Look up WatchObj → `watch_json()` → start a conversion task that attaches server_id → return an `mpsc::Receiver<EventDataSimple>` |

The watch operation spawns a tokio task that converts `WatchResponseSimple` into `EventDataSimple` (attaching server_id), ensuring clients can detect server restarts.

### State management

`ConfigSyncServer` exposes `not_ready_kinds()` and `clear_all()` for its registered WatchObjs.
Processor registration/readiness and reload reset belong to `ProcessorRegistry`, ConfCenter, and
`ControllerRuntime`, not to fictional `is_all_ready`/`reset_for_relink` server methods.

## ConfigSyncGrpcServer

The gRPC service layer that implements the `ConfigSync` proto service; receives Gateway gRPC calls and routes them to ConfigSyncServer.

```rust
pub struct ConfigSyncGrpcServer {
    provider: Arc<dyn ConfigSyncServerProvider>,
}
```

ConfigSyncGrpcServer does not hold ConfigSyncServer directly; instead it dynamically obtains the latest instance through the `ConfigSyncServerProvider` trait, in order to support server-instance switching during reload.

### 4 RPC implementations

| RPC | Implementation |
|-----|---------|
| `GetServerInfo` | Returns server_id and supported_kinds (kinds of all registered WatchObjs) |
| `List` | Validate expected_server_id → look up the WatchObj for the kind → call `list_sync_cached()` (per-(kind, sync_version) serialized-bytes cache, single-flight fill) → return ListResponse |
| `Watch` | Validate expected_server_id → look up the WatchObj → call `watch_json()` → return a streaming WatchResponse |
| `WatchServerMeta` | Register the Gateway instance into ClientRegistry → push gateway_instance_count change-event stream |

When `expected_server_id` does not match the current `server_id`, returns a `WATCH_ERR_SERVER_ID_MISMATCH` error to inform Gateway to re-sync.

List and Watch use the shared `edgion-common::conf_sync::payload` codec. JSON over
32 MiB uses Zstd level 3 only if smaller; both raw and decoded payloads have a
512 MiB safety budget. The gRPC message budget adds 64 KiB for protobuf metadata.
`list_grpc_cached` caches the encoded result (including errors) in a per-snapshot
OnceCell; `list_sync_cached` remains raw JSON for federation. Admin redaction never
reads or populates either transport cache. Configuration Watch queues have capacity
one; metadata queues are unchanged. The codec runs at most two blocking jobs per
process and retains the permit until each job finishes, even after cancellation.
The raw snapshot is still serialized before encoding; this budget is not a bound
on process memory or on all transient allocations. The TLS accept layer
bounds in-flight handshakes at 256 and applies 10-second handshake / 30-second handoff timeouts.

## ConfigSyncServerProvider

A provider trait that lets the gRPC service dynamically obtain the latest ConfigSyncServer instance.

```rust
pub trait ConfigSyncServerProvider: Send + Sync {
    fn config_sync_server(&self) -> Option<Arc<ConfigSyncServer>>;
}
```

This design solves server-instance switching during reload: when Controller performs a reload, it creates a new ConfigSyncServer instance (new server_id, new WatchObj set), while the gRPC service layer transparently switches to the new instance through the provider, without restarting the gRPC service.

A `None` return indicates that ConfigSyncServer is not yet ready (e.g., Controller is initializing or reloading); the gRPC layer returns `Status::unavailable`.

## Registration timing requirement

ConfigSyncServer must wait for all phased processors to finish registration before it can be published.

In Kubernetes mode, Phase 1 base resources (Gateway, Service, EndpointSlice, etc.) start before Phase 2 route/TLS/plugin resources. If ConfigSyncServer is published immediately after Phase 1 is ready, Gateway might receive an incomplete `supported_kinds` and List calls for Phase 2 resources will return errors:

```
Failed to list resources: Unknown kind: HTTPRoute
Failed to list resources: Unknown kind: GRPCRoute
Failed to list resources: Unknown kind: EdgionTls
```

This is a control-plane readiness issue, not an unsupported resource type. Gateway may continue serving traffic with the previously cached snapshot.

## WatchObj trait

`ConfigSyncServer` manages per-kind caches uniformly through the object-safe `WatchObj` trait, which `ServerCache<T>` implements. The trait definition and its method semantics are canonical in [01-controller/07-cache-server.md](../01-controller/07-cache-server.md#watchobj-trait); they are not repeated here.

## ClientRegistry

Tracks distinct connected Gateway IDs and maintains gateway_instance_count information,
with a minimum reported count of one. Each metadata RPC owns one non-cloneable
registration guard; overlapping streams with the same ID share a counted entry.
Only the last guard removes it, including early return and task cancellation
(ARCH-118). First/last registration transitions notify watchers. This ownership
contract does not change the stream's bounded send backpressure or establish a
count-delivery deadline.

Metadata observation (ARCH-119) starts with the count captured for the initial
RPC event. Its baseline advances only after that exact snapshot is enqueued,
not after an unrelated registry read. Each loop constructs the actual Tokio
`Notified` before scanning the count, and keeps lifecycle branches in the same
select as count publication. The next loop rescans after a bounded send, so a
change during backpressure is projected once capacity returns. This is latest
state projection, not every intermediate membership state or remote acknowledgement.

Main uses:
- **Cluster-level rate limiting**: Gateway receives `gateway_instance_count` via WatchServerMeta to compute cluster-level rate-limit quotas (per-instance quota = total cluster quota / instance count)
- **Instance monitoring**: Controller can view the list of currently connected Gateway instances and their state
- **Change notification**: when Gateway instances come online/offline, push an updated `gateway_instance_count` to all connected Gateways via the WatchServerMeta stream

## ServerCache integration

ResourceProcessor is the data source for ServerCache, which feeds ConfigSyncServer's watch streams. The full data flow (ConfCenter → ResourceProcessor → ServerCache → Notify → Watcher Task → ConfigSyncServer → gRPC → Gateway) and the detailed ServerCache implementation (EventStore, ring buffer, sync_version mechanism) are documented canonically in [01-controller/07-cache-server.md](../01-controller/07-cache-server.md#full-data-flow).

## Module structure

```
edgion-controller/src/conf_sync/
├── conf_server/
│   ├── mod.rs                  # Module exports
│   ├── config_sync_server.rs   # ConfigSyncServer core logic
│   ├── grpc_server.rs          # ConfigSyncGrpcServer + ConfigSyncServerProvider
│   ├── client_registry.rs      # ClientRegistry implementation
│   └── traits.rs               # WatchObj trait + WatchResponseSimple
└── cache_server/
    ├── cache.rs                # ServerCache<T> generic cache
    ├── store.rs                # EventStore<T> ring event buffer
    └── types.rs                # WatcherEvent and other type definitions
```
