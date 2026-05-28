---
name: link-gateway-side
description: Gateway-side gRPC implementation — ConfigSyncClient, ClientCache, CacheEventDispatch, ConfHandler, ArcSwap lock-free reads, reconnection mechanism.
---

# Gateway-side implementation

The Gateway connects to the Controller's gRPC service via `ConfigSyncClient`, receives resource configuration, and writes it into the local `ClientCache<T>`. For each resource type, a `ConfHandler` turns the cache data into runtime proxy behavior, using ArcSwap for lock-free reads.

## ConfigSyncClient

Manages the gRPC connection to the Controller; handles List/Watch calls, version tracking, and reconnection.

```rust
pub struct ConfigSyncClient {
    config_client: Arc<ConfigClient>,
    conf_client_handle: ConfigSyncClientService<Channel>,
    client_id: String,
    client_name: String,
}
```

### Startup sync sequence

1. **Connection establishment**: connect to the Controller's gRPC port (default :50051) via tonic
2. **GetServerInfo**: fetch `server_id`, `supported_kinds`
3. **Per-kind List**: for each resource in `supported_kinds`, issue a List call and fetch the full snapshot
4. **Per-kind Watch**: use the `sync_version` returned by List as `from_version` and start the Watch stream
5. **WatchServerMeta**: register with the Controller's ClientRegistry and receive `gateway_instance_count` changes

### Reconnection mechanism

ConfigSyncClient triggers a reconnection in the following situations:

| Trigger condition | Behavior |
|---------|------|
| Watch stream receives `WATCH_ERR_SERVER_RELOAD` | Controller performed a reload, full relist |
| Watch stream receives `WATCH_ERR_SERVER_ID_MISMATCH` | Controller restarted, re-run GetServerInfo + full relist |
| gRPC connection broken | Reconnect with exponential backoff, re-establish connection and full sync |
| `server_id` changed | Full relist for all kinds |

After reconnecting, Gateway re-runs the full sync sequence (GetServerInfo → List → Watch) to ensure the configuration is fully consistent with the Controller.

## ClientCache\<T\>

One cache instance per resource type, storing resource data synced from the Controller.

```rust
pub struct ClientCache<T> where T: kube::Resource {
    pub(crate) cache_data: Arc<RwLock<CacheData<T>>>,
    pub(crate) grpc_client: Arc<AsyncRwLock<Option<ConfigSyncClientService<Channel>>>>,
    pub(crate) client_id: Arc<String>,
    pub(crate) client_name: Arc<String>,
}
```

### CacheData\<T\>

ClientCache internally holds `CacheData<T>` (RwLock-protected), which contains:
- A full resource snapshot (`HashMap<String, T>`)
- The current `sync_version`
- `server_id` (used to detect Controller restarts)
- `CompressEvent`: a queue of pending incremental events (inside the `handler`)

#### Event-batch compression background task

When `set_conf_processor` is called, `CacheData` starts a background Tokio task for that resource type. The task sleeps in `notify.notified().await` and only wakes when `add_compress_event` calls `notify_one()`; on wake it waits a short debounce window (`BATCH_DEBOUNCE`, **100ms**) so further events in the same burst are merged, then takes the write lock once and dispatches the accumulated `CompressEvent`s to `ConfHandler.partial_update()`.

**Design intent**: prevent a burst of K8s resource changes (e.g., rolling updates, batch ConfigMap writes) from invoking `partial_update` at high frequency and triggering excessive ArcSwap rebuilds. The 100ms debounce window merges multiple incremental events into a single `partial_update`, significantly reducing the rate of ArcSwap stores. The Notify-driven wakeup also avoids steady-state write-lock contention: when no events arrive the task stays parked and never touches the `RwLock`.

### DynClientCache

A type-erased uniform dispatch interface that lets ConfigSyncClient manage ClientCaches of different types in a uniform way:

```
ConfigSyncClient
  └── Vec<Arc<dyn DynClientCache>>   // Indexed by kind
       ├── ClientCache<Gateway>
       ├── ClientCache<HTTPRoute>
       ├── ClientCache<GRPCRoute>
       └── ...
```

## Event dispatch (CacheEventDispatch)

The `CacheEventDispatch<T>` trait defines the cache event handling interface:

```rust
pub trait CacheEventDispatch<T> {
    fn apply_change(&self, change: ResourceChange, resource: T) where T: Send + 'static;
    fn set_ready(&self);
}
```

ClientCache implements this trait, mapping gRPC events (from WatchResponse on the Watch stream) to ResourceChange operations:

| Watch event | ResourceChange | Handling logic |
|-----------|---------------|---------|
| Init start | `InitStart` | Log it, prepare to receive initialization data |
| Init data | `InitAdd` | Write to the CacheData snapshot (does not trigger ConfHandler) |
| Init done | `InitDone` | Mark cache ready, call ConfHandler.full_set() |
| Runtime add | `EventAdd` | Update CacheData + call ConfHandler.partial_update(add) |
| Runtime update | `EventUpdate` | Update CacheData + call ConfHandler.partial_update(update) |
| Runtime delete | `EventDelete` | Remove from CacheData + call ConfHandler.partial_update(remove) |

Event dispatch also compresses event batches — multiple consecutive incremental events can be merged into a single partial_update call, reducing the rebuild frequency of runtime structures.

## ConfHandler

The configuration handler for each resource type, defined in `src/core/common/conf_sync/traits.rs`:

```rust
pub trait ConfHandler<T>: Send + Sync {
    fn full_set(&self, data: &HashMap<String, T>);
    fn partial_update(&self, add: HashMap<String, T>, update: HashMap<String, T>, remove: HashSet<String>);
}
```

### full_set

Called when initialization sync completes (after InitDone), with a full resource snapshot. Based on this, ConfHandler builds a complete runtime structure and atomically replaces it via ArcSwap.

### partial_update

Called on runtime incremental changes; receives three groups of data:
- `add`: newly added resources
- `update`: updated resources
- `remove`: set of removed resource keys

Based on this, ConfHandler incrementally updates the runtime structure. For complex resources (e.g., routes), partial_update may trigger a partial or full rebuild depending on the scope of the change.

### Preparse

After receiving changes, ConfHandler triggers preparse, transforming declarative Kubernetes-style resources into efficient runtime data structures. For example:
- HTTPRoute → route matching tree
- EdgionPlugins → plugin execution chain
- EdgionTls → TLS certificate and key configuration
- Gateway → listener bindings and port mappings

## ArcSwap lock-free reads

Gateway uses ArcSwap to enable lock-free reads of configuration data:

```
Write path (Tokio runtime):
  gRPC Watch → CacheEventDispatch → ConfHandler → ArcSwap.store()

Read path (Pingora proxy layer):
  Request handling → ArcSwap.load() → read the latest configuration
```

This design ensures:
- **Writes do not block reads**: configuration updates complete via atomic pointer replacement; readers always see a consistent snapshot
- **Zero-copy reads**: Pingora threads share the data via `Arc` reference counting, no cloning needed
- **Lock-free**: the read path involves no lock contention, suitable for a high-concurrency proxy scenario

## WatchServerMeta

Gateway receives cluster-level metadata via the WatchServerMeta RPC:

```rust
message ServerMetaEvent {
    string server_id = 1;
    uint32 gateway_instance_count = 2;
    uint64 timestamp = 10;
}
```

`gateway_instance_count` reflects the total number of Gateway instances currently connected to the Controller. Gateway uses this information for cluster-level awareness; a typical use is cluster-level rate limiting — per-instance quota = total cluster quota / `gateway_instance_count`.

When new Gateway instances come online or go offline, the Controller's ClientRegistry pushes the updated count to all connected instances.

## ConfigClient

Aggregates the ClientCaches of all resource types and provides a type-safe query interface:

```
ConfigClient
  ├── ClientCache<Gateway>
  ├── ClientCache<GatewayClass>
  ├── ClientCache<HTTPRoute>
  ├── ClientCache<GRPCRoute>
  ├── ClientCache<TCPRoute>
  ├── ClientCache<TLSRoute>
  ├── ClientCache<UDPRoute>
  ├── ClientCache<EdgionTls>
  ├── ClientCache<EdgionPlugins>
  ├── ClientCache<EdgionStreamPlugins>
  ├── ClientCache<PluginMetaData>
  ├── ClientCache<BackendTLSPolicy>
  ├── ClientCache<EdgionGatewayConfig>
  ├── ClientCache<LinkSys>
  ├── ClientCache<EdgionAcme>
  └── ClientCache<Service/EndpointSlice>
```

ConfigClient also provides an `is_ready()` method, checking whether all required resources have completed initial sync. The Gateway only starts accepting traffic after ConfigClient is ready.

## Module structure

```
src/core/gateway/conf_sync/
├── conf_client/
│   ├── mod.rs              # Module exports
│   ├── config_client.rs    # ConfigClient aggregation layer
│   └── grpc_client.rs      # ConfigSyncClient gRPC client
└── cache_client/
    ├── mod.rs              # Module exports
    ├── cache.rs            # ClientCache<T>, DynClientCache
    ├── cache_data.rs       # CacheData<T> data store
    └── event_dispatch.rs   # CacheEventDispatch implementation
```
