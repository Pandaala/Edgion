---
name: controller-cache-server
description: ServerCache + EventStore — generic in-memory cache, ring event buffer, monotonically increasing sync_version, WatchObj trait, ConfigSyncServer architecture.
---

# CacheServer (In-memory Cache and Configuration Sync)

ServerCache and EventStore form the Controller's in-memory cache layer. The ResourceProcessor writes processed resources into ServerCache, which then pushes changes to gRPC watcher clients (Gateway instances) via an event-notification mechanism.

## ServerCache\<T\>

A generic in-memory cache, one instance per resource type, created and held by the corresponding `ResourceProcessor<K>`.

```rust
pub struct ServerCache<T: ResourceMeta + Resource + Send + Sync> {
    ready: RwLock<bool>,                          // initialization-complete flag
    store: Arc<RwLock<EventStore<T>>>,             // event store
    watchers: RwLock<Vec<WatchClient<T>>>,          // pending watch requests
    notify: Arc<Notify>,                            // broadcast notification (wakes all watcher tasks)
}
```

### Lifecycle

1. **Create**: `ServerCache::new(capacity)` — initializes the ring buffer; state is not-ready
2. **Init writes**: `apply_change(InitAdd, resource)` — synchronously writes to EventStore (does not trigger watcher notifications)
3. **Mark ready**: `set_ready()` — marks the cache as ready, allowing gRPC clients to connect
4. **Runtime writes**: `apply_change(EventUpdate/EventDelete, resource)` — writes the event and notifies all watchers
5. **Relink**: `clear()` + `set_not_ready()` — clears data and waits for a fresh initialization

### Data Operations

| Method | Description |
|---|---|
| `list()` / `list_owned()` | Return a snapshot of all resources and the current sync_version |
| `get_by_key(key)` | Get a single resource by `"namespace/name"` key |
| `watch(client_id, client_name, from_version)` | Start a watcher task; returns an mpsc::Receiver that continuously delivers changes |
| `apply_change(change, resource)` | Perform the corresponding operation based on the ResourceChange type |

### sync_version

`sync_version` is a globally monotonically increasing version number (produced by `utils::next_resource_version()`), independent of the K8s `resourceVersion`. Each write assigns a new version, which is used for:

- Watcher incremental sync: the client reports `from_version`; the server returns all events newer than that version
- Stale detection: when the client version is too old (events have already been overwritten in the ring buffer), a `TooOldVersion` error is returned
- Out-of-order protection: events with a version below the current `sync_version` are dropped (Stage A protection)

### Watcher Tasks

`start_watcher_task()` spawns an independent tokio task per watch request:

```
loop {
    1. Fetch events newer than from_version from EventStore
    2. If new events exist → send to client, advance from_version
    3. If the version skipped but no events were available → events were lost; send WATCH_ERR_EVENTS_LOST
    4. If no new events → wait on notify.notified() or sender.closed()
}
```

Watcher exit conditions: receiver disconnects (gRPC connection closed), EventStore returns an error, or events were lost.

## EventStore\<T\>

A ring buffer of recent resource-change events that also maintains a full resource snapshot.

```rust
pub struct EventStore<T> {
    capacity: usize,                    // ring-buffer capacity (min 10, default 200)
    cache: Vec<Option<WatcherEvent<T>>>, // ring buffer
    start_index: usize,                 // start of valid events
    end_index: usize,                   // next write position
    sync_version: u64,                  // current maximum version
    expire_sync_version: u64,           // smallest expired version
    data: HashMap<String, T>,           // full resource snapshot (key → latest resource)
}
```

### Event Types

`WatcherEvent<T>` has three variants — `Add`, `Update`, `Delete` — each carrying the `sync_version` and resource data.

### Ring-buffer Mechanics

- On write, `end_index` advances; when the buffer is full, `start_index` advances (overwriting the oldest event)
- The `sync_version` of an overwritten event is recorded as `expire_sync_version`
- When a client requests `from_version < expire_sync_version`, a `TooOldVersion` error is returned, triggering a full client re-sync

### Out-of-order Protection (Stage A)

Before writing, `apply_event()` checks: if `incoming_sync_version < current_sync_version`, the event is dropped and a warning is logged. This prevents stale data from overwriting newer data due to tokio scheduler reordering.

### Query Interface

| Method | Description |
|---|---|
| `get_events_from_sync_version(from)` | Return all events newer than `from` plus the current version |
| `snapshot_owned()` | Return a full resource snapshot (cloned) |
| `get_by_key(key)` | Get a single resource by key |
| `init_add(version, resource)` | Init-phase write (does not enter the ring buffer) |

## WatchObj trait

An object-safe list/watch interface implemented by `ServerCache<T>`, allowing `ConfigSyncServer` to manage caches of different types uniformly.

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

All methods involve serialization (JSON), so a trait object is a reasonable choice — serialization overhead is far greater than virtual-dispatch overhead. `ServerCache<T>` returns a JSON string + `sync_version` from `list_json()` and a `WatchResponseSimple` (event stream as JSON strings) from `watch_json()`.

## ConfigSyncServer

The gRPC configuration sync server. It owns all WatchObjs and provides a unified list/watch API.

```rust
pub struct ConfigSyncServer {
    server_id: RwLock<String>,                              // server instance ID (millisecond timestamp)
    watch_objects: RwLock<HashMap<String, Arc<dyn WatchObj>>>, // WatchObjs registered by kind
    client_registry: Arc<ClientRegistry>,                    // registry of connected Gateway instances
}
```

### server_id

A millisecond-timestamp string generated at startup, used by clients to detect server restarts. When the Gateway sees a `server_id` different from the previous one, it triggers a full re-list. On relink, `regenerate_server_id()` produces a new ID.

### WatchObj Registration

The ResourceProcessor registers itself during initialization via `register_watch_obj(kind, Arc<ServerCache<T>>)`. Once all registrations complete and they are ready (`is_all_ready()` returns true), the gRPC service starts serving data.

### list/watch Operations

| Operation | Flow |
|---|---|
| `list(kind)` | Look up the WatchObj → call `list_json()` → return `ListDataSimple { data, sync_version, server_id }` |
| `watch(kind, client_id, ...)` | Look up the WatchObj → call `watch_json()` → start a transformation task that attaches `server_id` → return `mpsc::Receiver<EventDataSimple>` |

The watch operation spawns a tokio task that converts `WatchResponseSimple` to `EventDataSimple` (attaching `server_id`) so clients can detect server restarts.

### State Management

| Method | Description |
|---|---|
| `is_all_ready()` | All WatchObjs are ready |
| `not_ready_kinds()` | Return the list of kinds that are not yet ready |
| `set_all_not_ready()` | Mark all caches as not-ready on relink |
| `clear_all()` | Clear data in all caches |
| `reset_for_relink()` | Full reset: set_all_not_ready + clear_all + regenerate_server_id |

## Full Data Flow

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
  ServerCache.apply_change()              ← assign sync_version, write to EventStore
        │
        ▼
  Notify.notify_waiters()                 ← wake all watcher tasks
        │
        ▼
  Watcher Task → mpsc::channel            ← send incremental events
        │
        ▼
  ConfigSyncServer.watch()                ← attach server_id
        │
        ▼
  gRPC Stream → Gateway                  ← Gateway instance receives configuration changes
```
