---
name: link-gateway-side
description: Gateway-side gRPC implementation — ConfigSyncClient, ClientCache, CacheEventDispatch, ConfHandler, ArcSwap lock-free reads, reconnection mechanism.
---

# Gateway-side implementation

The Gateway connects to the Controller's gRPC service via `ConfigSyncClient`, receives resource configuration, and writes it into the local `ClientCache<T>`. For each resource type, a `ConfHandler` turns the cache data into runtime proxy behavior, using ArcSwap for lock-free reads.

## ConfigSyncClient

Manages the gRPC connection to the Controller; handles List/Watch calls, version tracking, and reconnection.

The client separates the installed connection from candidate recovery. The installed
snapshot combines address, address-list index, service handle, certificate validity,
and reload generation.
The channel manager owns its publication boundary and the candidate traversal state.
This prevents a meta reconnect from mixing an old address with a new handle or token.

### Startup sync sequence

1. **Channel preparation**: prepare a lazy tonic channel for the initially shuffled
   Controller address (default port :12151); this does not establish remote readiness
2. **GetServerInfo**: fetch `server_id`, `supported_kinds` within the bounded startup
   retry budget; failed multi-address startup uses the same candidate recovery as runtime
3. **Per-kind loops**: concurrently start one loop for every supported kind that has a Gateway cache
4. **List then Watch**: each loop resets its cache from one List snapshot, marks it ready, then
   Watches from the returned `sync_version`
5. **WatchServerMeta**: register with the Controller's ClientRegistry and receive `gateway_instance_count` changes

### Reconnection mechanism

Initial, recovery and certificate-reload clients share the same finite receive
budget from `conf_sync::payload`. List and Watch decode the explicit payload
encoding and exact uncompressed length before JSON parsing. Unknown encodings,
oversized output, corrupt/truncated Zstd frames, extra frames and trailing bytes
fail decoding. Cancellation observed during or immediately after asynchronous
decoding discards the result before publication or Backend clearing. This does
not make the existing synchronous JSON parsing/publication atomic with a
concurrent channel swap. Invalid envelopes preserve generic last-known-good state;
Backend invalid-envelope handling follows its invalid-JSON withdrawal policy.

On-demand recovery builds a candidate channel, then calls `GetServerInfo` on that
exact handle before installing it. Controller provider availability is the existing
serving gate for leader-only and all-serve deployments. This probe creates no client
registration; only the active `WatchServerMeta` session registers the Gateway.
The probe is a point-in-time readiness observation, not a leadership lease.

Candidate traversal and the installed address are different state. A local TLS or
channel-construction error preserves the candidate and retries with a bounded pause.
All immutable configured URI strings are syntax-checked at startup, preventing a
permanently malformed later candidate from blocking traversal after startup.
An actual failed remote RPC advances traversal, including a later revisit of the
original address, without replacing any installed client. Full failed remote laps
receive capped jittered backoff. A successful unary probe does not reset the failure
history: the first valid event of the current meta session does that. The existing
leader dwell and single-address reconnect behavior remain.

Network probing holds no swap lock and has a deadline covering the complete unary
response. Its result carries the installed snapshot identity and a selection revision;
publication rejects stale results after a TLS reload, another commit, or another
candidate transition. Meta processing likewise ignores results from retired generations.

Publication is a finite, cancellation-safe operation on the channel manager. It
updates every per-kind cache client, publishes the coherent installed snapshot and
global reload token, and cancels the previous generation last. A caller being
cancelled cannot stop the operation between cache replacements. This orders channel
publication and watcher notification; it does not make configuration across resource
kinds atomic or retract a callback that already applied an event.

Certificate reload selects the current address within the same publication boundary,
validates local material, and publishes the new identity without depending on remote
availability. An old candidate probe cannot reinstall a pre-reload identity. The
file watcher's accepted `(mtime, length)` trigger policy is unchanged.

Certificate validity is parsed from the same bytes used to build each initial,
recovery, or reload Channel. A `PreparedClient` carries both the handle and
`LeafValidity` through probing and publication. `/metrics` reads the installed
snapshot and invokes the shared expiry reporter through `GatewayMetrics`; it never
reads the mounted Secret files. Rejected or stalled reloads keep the old expiry,
and a stale candidate cannot publish its metadata. Explicit plaintext carries no
certificate validity and exports no client-expiry series. Auto-CA initialization
failures remain observable through the existing retry diagnostics before an
identity is installed. Prometheus compares the exported timestamp with `time()`;
no independent Gateway expiry polling task is needed.

ConfigSyncClient triggers a reconnection in the following situations:

| Trigger condition | Behavior |
|---------|------|
| Watch stream receives `WATCH_ERR_SERVER_RELOAD` | Controller replaced the serving resource epoch; perform a full relist |
| Watch/server generation changes | End that loop's stream and relist the kind |
| gRPC connection broken | Reconnect with exponential backoff and List before the next Watch |
| TooOld / unexpected version / not-ready | Relist the affected kind |
| Generic Watch JSON/envelope/resource decode failure | Stop at the first failure, retain valid prefix events and readiness, back off and List before the next Watch |

Each per-kind retry always performs List before Watch and therefore does not resume from a retained
pre-disconnect cache version. `GetServerInfo` is not rerun for every per-kind retry.
Generic List decode failure retains existing configuration and retries; readiness
means initial configuration is available, not that it matches the newest server
state. EdgionBackend retains its stricter withdrawal behavior. See
[decode recovery and diagnostics](../../04-review/conf-sync/regression-guards/watch-decode-error-relist.md).

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
- A full resource snapshot (`HashMap<String, Arc<T>>`)
- The last List/reset `sync_version` (not an applied-Watch cursor)
- `CompressEvent`: a queue of pending incremental events (inside the `handler`)

The Watch loop retains `server_id` separately to detect Controller restarts.

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
    fn apply_change(&self, change: ResourceChange, resource: T, sv: Option<u64>) where T: Send + 'static;
    fn set_ready(&self);
}
```

`ResourceChange` initialization variants are used while building the Controller's `ServerCache`.
They are not wire frames in a List response. Gateway full sync instead deserializes List's single
JSON Vec and calls `CacheData::reset`, which invokes `ConfHandler.full_set`; the cache is marked
ready after that List succeeds. Runtime Watch changes go through the generic or
Backend-specific payload decoder, update the cache, and enter the debounce path below.

| Watch event | ResourceChange | Handling logic |
|-----------|---------------|---------|
| Runtime add | `EventAdd` | Update CacheData + call ConfHandler.partial_update(add) |
| Runtime update | `EventUpdate` | Update CacheData + call ConfHandler.partial_update(update) |
| Runtime delete | `EventDelete` | Remove from CacheData + call ConfHandler.partial_update(remove) |

Event dispatch also compresses event batches — multiple consecutive incremental events can be merged into a single partial_update call, reducing the rebuild frequency of runtime structures.

## ConfHandler

The configuration handler for each resource type, defined in `edgion-common/src/conf_sync/traits.rs`:

```rust
pub trait ConfHandler<T>: Send + Sync {
    fn full_set(&self, data: &HashMap<String, Arc<T>>);
    fn partial_update(
        &self,
        add: HashMap<String, Arc<T>>,
        update: HashMap<String, Arc<T>>,
        remove: HashSet<String>,
    );
}
```

### full_set

Called by `CacheData::reset` after a successful List snapshot is decoded. Resource values are
shared as `Arc<T>` to avoid deep cloning across cache and handler stores.

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
  ├── ClientCache<EdgionConfigData>
  ├── ClientCache<BackendTLSPolicy>
  ├── ClientCache<EdgionGatewayConfig>
  ├── ClientCache<LinkSys>
  ├── ClientCache<EdgionAcme>
  └── ClientCache<Service/EndpointSlice>
```

ConfigClient also provides an `is_ready()` method, checking whether all required resources have completed initial sync. The Gateway only starts accepting traffic after ConfigClient is ready.

## Module structure

```
edgion-gateway/src/conf_sync/
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
