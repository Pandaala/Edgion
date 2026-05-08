---
name: kubernetes-resource-controller
description: Kubernetes ResourceController lifecycle — Reflector watching, Init/Runtime phases, ResourceProcessor 11-step pipeline, status writeback, 410 Gone handling.
---

# ResourceController Lifecycle

Each K8s resource type is managed by a dedicated `ResourceController<K>`. All instances run independently and in parallel.

## Architecture Overview

```
KubernetesController.run()
├── spawn::<HTTPRoute, _>(handler)      → ResourceController<HTTPRoute>
├── spawn::<Gateway, _>(handler)        → ResourceController<Gateway>
├── spawn::<Service, _>(handler)        → ResourceController<Service>
└── ... (~20 resource types, all independent)
```

Internal structure of a ResourceController:

```
ResourceController<K>
├── kind: &'static str
├── client: kube::Client
├── watcher_config: watcher::Config
├── api_scope: ApiScope                  # Namespaced(watch_mode) | ClusterScoped
├── processor: Arc<ResourceProcessor<K>> # holds ServerCache + Workqueue + Handler
├── namespace_filter: Option<Vec<String>>
├── shutdown_signal: Option<ShutdownSignal>
├── relink_signal: Option<RelinkSignalSender>
└── leader_handle: Option<LeaderHandle>
```

Startup steps:
1. Create `ResourceProcessor<K>` (containing `ServerCache<K>` + `Workqueue`)
2. Register it into `PROCESSOR_REGISTRY`
3. Call `run_namespaced()` or `run_cluster_scoped()` → `run_with_api(api)`
4. Enter the Init phase → Runtime phase

## Init Phase (Steps 1-6)

```
run_with_api(api)
│
├── Step 1: Create the Reflector store + watcher stream and inject KubeRawStore
│   store, writer = reflector::store()
│   processor.set_raw_store(KubeRawStore::new(store.clone()))  # ← injected immediately, before any InitApply
│   watcher_stream = watcher(api, watcher_config)
│   stream = reflector(writer, watcher_stream)  # merged event stream
│
├── Step 2: Event::Init
│   └── processor.on_init()                     # mark initialization started
│       └── init_timer = InitSyncTimer::start()
│
├── Step 3-5: Event::InitApply(obj)  (repeated N times, once per LIST result)
│   ├── namespace_filter check
│   ├── processor.on_init_apply(obj, None)
│   │   └── Full ResourceProcessor 11-step pipeline
│   ├── init_count += 1
│   └── if status_changed && can_write_status:
│       └── persist_k8s_status()                # leader guard
│
└── Step 6: Event::InitDone
    ├── processor.on_init_done()                # mark ready
    └── spawn_worker()                          # start Workqueue worker
```

The Init phase processes K8s API LIST results; each object triggers `InitApply`.
Status writebacks during this phase are synchronous (awaited directly inside the event loop) since the worker is not yet started.

## Runtime Phase (Steps 7-8)

```
Step 7-8: Runtime Phase (WATCH)
├── Event::Apply(obj)
│   ├── namespace_filter check
│   ├── processor.on_apply(obj)       → enqueue key
│   └── (special case: Apply arriving before init_done is processed directly + status written)
│
├── Event::Delete(obj)
│   └── processor.on_delete(obj)      → enqueue key
│
└── Worker loop (spawn_worker):
    ├── dequeue work item             # blocks; key released from pending
    ├── get obj from reflector store
    ├── processor.process_work_item(obj, store)
    └── if status_changed && can_write_status:
        └── persist_k8s_status()      # leader guard
```

The Runtime phase uses a Go-operator-style workqueue: every event (Apply/Delete) first enqueues a key,
then the worker uniformly dequeues and processes it. The worker fetches the latest object state from the reflector store,
so even if multiple events arrive in a short window, only the latest state is processed once.

> **KubeRawStore note**: `KubeRawStore` wraps the reflector's `Store<K>` (internally an `Arc<DashMap>`),
> sharing the same underlying data with `writer`. Cloning only copies the Arc pointer — no extra memory cost.
> It contains every K8s object, unaffected by handler filters such as `gatewayClassName`,
> and is used by cross-resource lookups like `lookup_gateway` / `lookup_service_cluster_ip`.

## ResourceProcessor 11-step Pipeline

Each resource goes through the full processing pipeline (`process_resource`):

```
process_resource(obj):
├── Step 1.  Namespace filter check      — In multi-namespace mode, filter out unrelated namespaces
├── Step 2.  Handler filter              — Business filter (e.g. gateway_class match)
├── Step 3.  Clean metadata              — Remove managedFields, blocked annotations
├── Step 4.  Validate                    — Schema + semantic validation; collect errors
├── Step 5.  Preparse                    — Build runtime structures (e.g. parse host/path matching rules)
├── Step 6.  Parse                       — Resolve references, update caches (Secret/Service ref registration)
├── Step 7.  Extract old status          — Extract the old status from the current object (for change comparison)
├── Step 8.  Update status               — Compute the new status (Gateway API conditions)
├── Step 9.  Check if status changed     — Compare serialized JSON strings
├── Step 10. on_change                   — Notify dependent resources (cross-resource requeue)
└── Step 11. Save to ServerCache         — Save into the gRPC sync cache
```

Steps 4-6 are the responsibility of the concrete `ProcessorHandler<K>` implementation; each resource type has its own Handler.
Steps 7-9 use unified status extraction and comparison logic.
Cross-resource requeue in Step 10 is implemented via `PROCESSOR_REGISTRY.requeue()`.
After Step 11 writes to `ServerCache`, the gRPC Watch stream automatically pushes the change to the Gateway.

## persist_k8s_status — Status Writeback

Status is written back via the K8s API status subresource:

```rust
async fn persist_k8s_status<K>(
    client: &Client,
    api_scope: &ApiScope,
    namespace: Option<&str>,
    name: &str,
    status_value: &serde_json::Value,
) -> Result<(), kube::Error>
```

Uses `DynamicObject` + JSON Merge Patch to write the `/status` subresource.
`DynamicObject` is chosen over a generic `Api<K>` to avoid Scope generic-constraint issues
(the same function must handle both Namespaced and ClusterScoped resources).

### Leader Guard

Every `persist_k8s_status()` call is gated by `leader_handle`:

```rust
let can_write_status = leader_handle.as_ref().is_none_or(|h| h.is_leader());
if status_changed && can_write_status {
    persist_k8s_status::<K>(...).await;
}
```

The same logic is applied at three checkpoints:

| Checkpoint | Location | Description |
|------------|----------|-------------|
| Init phase | `Event::InitApply` processing | Status writeback for LIST results |
| Init edge case | `Event::Apply` arriving before init_done | Status writeback under race conditions |
| Runtime worker | Work-item processing in `spawn_worker` | Regular runtime status writeback |

When `leader_handle` is `None` (FileSystem mode), `is_none_or` returns `true` and writes are always allowed.

### Status Change Detection

Status changes are determined by comparing the serialized JSON strings:

```rust
enum StatusExtractResult {
    Present(String),       // status field is present and non-null; contains serialized JSON
    Empty,                 // status is null or empty
    SerializationError,    // serialization failed
}
```

Only an actual status change triggers a K8s API call, avoiding unnecessary patch requests.
The comparison uses JSON strings rather than struct PartialEq, because different resource types have different status structures, and a unified JSON-string comparison is the most general approach.

## Workqueue Mechanism

### Structure

```rust
pub struct Workqueue {
    kind: &'static str,
    queue: Mutex<VecDeque<WorkItem>>,       // FIFO queue
    pending: Mutex<HashMap<String, PendingItem>>,  // dedup set
    notify: Notify,                         // wake the worker
    config: WorkqueueConfig,
}
```

### Dedup

```
enqueue("default/my-route"):
├── if "default/my-route" in pending → skip (already queued)
└── else → add to pending + push to queue + notify worker
```

### Dirty Requeue

```
dequeue():
├── pop from queue
├── remove from pending    ← key released here
└── return WorkItem

# While processing "default/my-route":
# If K8s sends a new event for that key → enqueue succeeds
# Because the key has been removed from pending
```

### WorkqueueConfig

```rust
pub struct WorkqueueConfig {
    pub default_requeue_delay: Duration, // delay for cross-resource requeue
    pub max_trigger_cycles: usize,       // max trigger-chain cycle count
    pub max_trigger_depth: usize,        // max trigger-chain total length (safety cap)
}
```

> **No exponential-backoff retry**: status-write failures (such as transient K8s API unavailability) rely on a full rebuild after the watcher reconnects to self-heal. Reliably distinguishing transient jitter from permanent failure is impossible, so explicit retry has limited value.

### TriggerChain (Cascade Tracking)

Cross-resource requeue carries a `TriggerChain` to detect cycles and bound depth:

```rust
pub struct TriggerChain {
    entries: Vec<TriggerEntry>,  // [(kind, key), ...]
}
```

The chain is extended on each cross-resource requeue, and is checked against `max_trigger_cycles` or for cycles.
Once the limit is exceeded, propagation stops and a warning is logged.

## 410 Gone Handling

When the K8s API server's watch bookmark expires, the watcher receives a 410 Gone error,
causing the Reflector to reconnect and re-LIST.

`ResourceController` detects `Event::Init` reappearing after `init_done = true` and treats this as a watcher reconnect:

```rust
Event::Init => {
    if init_done {
        // Watcher reconnecting (possible 410 Gone)
        relink_signal.try_send(RelinkReason::WatcherReconnected);
        break;  // exit the current ResourceController
    }
}
```

After sending the `RelinkReason::WatcherReconnected` signal, the upper-level `KubernetesController` receives a `ControllerExit(Relink)` event,
triggering a full Controller rebuild:
clear `PROCESSOR_REGISTRY` → recreate every `ResourceController` → re-LIST + WATCH.

This is a conservative but safe strategy: any single resource's watcher reconnect triggers a full rebuild,
avoiding the risk of partial cache being out of sync with the K8s API.

## Key Files

- `src/core/controller/conf_mgr/conf_center/kubernetes/resource_controller.rs` — `ResourceController<K>`, `ApiScope`, `RelinkReason`
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/processor.rs` — `ResourceProcessor<K>`, `ProcessorObj` trait
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handler.rs` — `ProcessorHandler<K>` trait
- `src/core/controller/conf_mgr/sync_runtime/workqueue.rs` — `Workqueue`, `WorkItem`, `WorkqueueConfig`
- `src/core/controller/conf_mgr/processor_registry.rs` — `PROCESSOR_REGISTRY`
- `src/core/controller/conf_mgr/conf_center/kubernetes/status.rs` — `persist_k8s_status`
