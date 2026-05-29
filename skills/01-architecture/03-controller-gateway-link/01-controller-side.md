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

A millisecond-timestamp string generated at startup, used by clients to detect server restarts. When the `server_id` Gateway sees changes, it triggers a full relist. On relink the `regenerate_server_id()` call generates a new ID.

### WatchObj registration

When ResourceProcessor initializes, it registers with ConfigSyncServer via `register_watch_obj(kind, Arc<ServerCache<T>>)`. Once all registrations are complete and ready (`is_all_ready()` returns true), the gRPC service starts serving data.

### list/watch operations

| Operation | Flow |
|------|------|
| `list(kind)` | Look up WatchObj → `list_json()` → return `ListDataSimple { data, sync_version, server_id }` |
| `watch(kind, client_id, ...)` | Look up WatchObj → `watch_json()` → start a conversion task that attaches server_id → return an `mpsc::Receiver<EventDataSimple>` |

The watch operation spawns a tokio task that converts `WatchResponseSimple` into `EventDataSimple` (attaching server_id), ensuring clients can detect server restarts.

### State management

ConfigSyncServer's readiness/relink lifecycle methods (`is_all_ready()`, `not_ready_kinds()`, `set_all_not_ready()`, `clear_all()`, `reset_for_relink()`) are documented canonically in [01-controller/07-cache-server.md](../01-controller/07-cache-server.md#state-management).

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
| `List` | Validate expected_server_id → look up the WatchObj for the kind → call `list_json()` → return ListResponse |
| `Watch` | Validate expected_server_id → look up the WatchObj → call `watch_json()` → return a streaming WatchResponse |
| `WatchServerMeta` | Register the Gateway instance into ClientRegistry → push gateway_instance_count change-event stream |

When `expected_server_id` does not match the current `server_id`, returns a `WATCH_ERR_SERVER_ID_MISMATCH` error to inform Gateway to re-sync.

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

Tracks connected Gateway instances and maintains gateway_instance_count information.

Main uses:
- **Cluster-level rate limiting**: Gateway receives `gateway_instance_count` via WatchServerMeta to compute cluster-level rate-limit quotas (per-instance quota = total cluster quota / instance count)
- **Instance monitoring**: Controller can view the list of currently connected Gateway instances and their state
- **Change notification**: when Gateway instances come online/offline, push an updated `gateway_instance_count` to all connected Gateways via the WatchServerMeta stream

## ServerCache integration

ResourceProcessor is the data source for ServerCache, which feeds ConfigSyncServer's watch streams. The full data flow (ConfCenter → ResourceProcessor → ServerCache → Notify → Watcher Task → ConfigSyncServer → gRPC → Gateway) and the detailed ServerCache implementation (EventStore, ring buffer, sync_version mechanism) are documented canonically in [01-controller/07-cache-server.md](../01-controller/07-cache-server.md#full-data-flow).

## Module structure

```
src/core/controller/conf_sync/
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
