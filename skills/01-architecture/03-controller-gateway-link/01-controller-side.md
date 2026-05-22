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

| Method | Description |
|------|------|
| `is_all_ready()` | All WatchObjs are ready |
| `not_ready_kinds()` | Returns the list of kinds that are not yet ready |
| `set_all_not_ready()` | Marks all caches not-ready on relink |
| `clear_all()` | Clears all cache data |
| `reset_for_relink()` | Full reset: set_all_not_ready + clear_all + regenerate_server_id |

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

In Kubernetes mode, Phase 1 base resources (Gateway, Service, Endpoints, etc.) start before Phase 2 route/TLS/plugin resources. If ConfigSyncServer is published immediately after Phase 1 is ready, Gateway might receive an incomplete `supported_kinds` and List calls for Phase 2 resources will return errors:

```
Failed to list resources: Unknown kind: HTTPRoute
Failed to list resources: Unknown kind: GRPCRoute
Failed to list resources: Unknown kind: EdgionTls
```

This is a control-plane readiness issue, not an unsupported resource type. Gateway may continue serving traffic with the previously cached snapshot.

## WatchObj trait

An object-safe list/watch interface implemented by `ServerCache<T>`, allowing ConfigSyncServer to manage caches of different types uniformly.

```rust
pub trait WatchObj: Send + Sync {
    fn kind_name(&self) -> &'static str;
    fn list_json(&self) -> Result<(String, u64), String>;
    fn watch_json(&self, client_id: String, client_name: String, from_version: u64)
        -> mpsc::Receiver<WatchResponseSimple>;
    fn is_ready(&self) -> bool;
    fn set_ready(&self);
    fn set_not_ready(&self);
    fn clear(&self);
}
```

All methods involve JSON serialization, so using a trait object is reasonable — the serialization overhead far exceeds virtual call overhead.

| Method | Description |
|------|------|
| `kind_name()` | Returns the resource type name (e.g., "Gateway", "HTTPRoute") |
| `list_json()` | Returns the full resources as a JSON string + the current sync_version |
| `watch_json()` | Spawns a watcher task and returns an `mpsc::Receiver` to continuously receive change events |
| `is_ready()` | Whether the cache has finished initialization |
| `set_ready()` / `set_not_ready()` | Toggle readiness state |
| `clear()` | Clears the cache data |

## ClientRegistry

Tracks connected Gateway instances and maintains gateway_instance_count information.

Main uses:
- **Cluster-level rate limiting**: Gateway receives `gateway_instance_count` via WatchServerMeta to compute cluster-level rate-limit quotas (per-instance quota = total cluster quota / instance count)
- **Instance monitoring**: Controller can view the list of currently connected Gateway instances and their state
- **Change notification**: when Gateway instances come online/offline, push an updated `gateway_instance_count` to all connected Gateways via the WatchServerMeta stream

## ServerCache integration

ResourceProcessor is the data source for ServerCache. The full data flow:

```
K8s API / FileSystem
        │
        ▼
  ConfCenter (watcher)
        │
        ▼
  ResourceProcessor.process_resource()    ← 11-step processing pipeline
        │
        ▼
  ServerCache.apply_change()              ← assigns sync_version, writes to EventStore
        │
        ▼
  Notify.notify_waiters()                 ← wakes all watcher tasks
        │
        ▼
  Watcher Task → mpsc::channel            ← sends incremental events
        │
        ▼
  ConfigSyncServer.watch()                ← attaches server_id
        │
        ▼
  gRPC Stream → Gateway                  ← Gateway instance receives configuration changes
```

The detailed implementation of ServerCache (EventStore, ring buffer, sync_version mechanism) is documented in [01-controller/07-cache-server.md](../01-controller/07-cache-server.md).

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
