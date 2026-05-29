---
name: controller-workqueue
description: Workqueue internals — dedup, dirty requeue, delayed enqueue, TriggerChain cycle detection.
---

# Workqueue Mechanism

## Design

The Workqueue follows a Go controller-runtime style design. Each resource type owns its own queue instance, and provides:

- **Dedup**: the same key exists in the queue at most once
- **Dirty requeue**: re-enqueueing during processing is allowed; updates are not lost
- **Delayed enqueue**: coalesced scheduling for cross-resource requeue
- **TriggerChain**: cascade causality tracking and cycle detection

There is intentionally **no** exponential-backoff retry; failed status writes self-heal on watcher relist (see the note under WorkqueueConfig below).

Source file: `src/core/controller/conf_mgr/sync_runtime/workqueue.rs`

## Data Structures

### Workqueue Core Fields

```text
Workqueue
├── name: String                          // queue name (used in logs and metrics)
├── tx: mpsc::Sender<String>             // ready-queue sender (only the key flows through the channel)
├── rx: Mutex<mpsc::Receiver<String>>    // ready-queue receiver (Mutex enforces single consumer)
├── pending: Arc<DashMap<String, PendingEntry>> // key → (enqueue_time, merged chain); also the dedup set
├── scheduled: Arc<DashSet<String>>       // set of keys in the delay queue (for dedup)
├── delay_tx: mpsc::Sender<DelayedItem>   // delay-queue sender
├── config: WorkqueueConfig               // configuration
└── metrics: Arc<WorkqueueMetrics>        // metrics
```

The ready queue uses `mpsc::channel` (bounded, capacity controlled by `config.capacity`). The delay queue receives delayed items through a separate `mpsc::channel` and uses a `BinaryHeap` (min-heap ordered by `ready_at`) in a background task to manage timers.

### WorkItem

```rust
pub struct WorkItem {
    pub key: String,                // "namespace/name" (namespaced) or "name" (cluster-scoped)
    pub enqueue_time: Instant,      // enqueue time
    pub trigger_chain: TriggerChain, // cascade trace chain
}
```

Two construction methods:
- `WorkItem::new(key)` — original event, empty chain
- `WorkItem::with_chain(key, chain)` — cross-resource requeue, carrying a causality chain

Only the `key` (a `String`) flows through the ready-queue channel. The `enqueue_time` and merged
`trigger_chain` live in the `pending: DashMap<String, PendingEntry>`; `dequeue()` reconstructs the
`WorkItem` by pairing the dequeued key with its `PendingEntry`.

## WorkqueueConfig

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `capacity` | `usize` | 5000 | Bounded channel capacity |
| `default_requeue_delay` | `Duration` | 100ms | Default delay for cross-resource requeue |
| `max_trigger_cycles` | `usize` | 5 | Maximum number of times the same (kind, key) may appear in a trigger chain |
| `max_trigger_depth` | `usize` | 20 | Maximum total depth of a trigger chain (safety net) |

> **Note**: exponential-backoff retry (`requeue_with_backoff`) has been removed. Status-write failures (e.g., transient K8s API unavailability) are self-healing: when the watcher reconnects after a disconnect, a full cache rebuild is triggered, which reprocesses everything and rewrites all statuses. Accurately distinguishing transient and permanent failures at this layer is impractical.

## Dedup Mechanism

**The two sets dedup independently and do not observe each other**:

- `enqueue()` checks only `pending`:

```text
enqueue("ns/my-route"):
  pending already contains "ns/my-route"?
    ├─ yes → skip, return false
    └─ no  → insert into pending → send to mpsc → return true
```

- `enqueue_after()` checks only `scheduled` (does not check `pending`):

```text
enqueue_after("ns/my-route", 100ms, chain):
  scheduled already contains "ns/my-route"?
    ├─ yes → skip, return false
    └─ no  → insert into scheduled → send to delay heap → return true
```

Therefore, the same key can simultaneously exist in both `pending` and `scheduled` (the two paths run in parallel). When the delayed item expires, the delay loop performs a second dedup check against `pending`: if `pending.insert` fails (an immediate item is already present), the delayed version is silently dropped.

This design is deliberate: processing is idempotent, so processing once more does not affect correctness; forcing cross-set mutual exclusion would only increase lock contention and false skips.

When the queue depth reaches 80% of capacity, a warning log is printed (once per 500 items).

## Dirty Requeue Pattern

Key design point: **no "processing" state is tracked**. A key is removed from `pending` immediately upon `dequeue()`, not when processing finishes.

```text
Timeline:
  t1: enqueue("ns/a")    → pending = {"ns/a"}
  t2: dequeue()           → pending = {}          // removed immediately
  t3: (during processing) enqueue("ns/a") → pending = {"ns/a"}  // re-enqueue allowed
  t4: done("ns/a")        → (no-op)
  t5: dequeue()           → pending = {}          // process the latest version
```

This guarantees that updates arriving during processing are not lost: even if a key is currently being processed, new changes can still be enqueued and processed afterwards.

The `done()` method is currently a no-op (only logs); the API is preserved for compatibility.

## Delayed Enqueue (enqueue_after)

Used for coalesced scheduling of cross-resource requeue. When a change to resource A needs to trigger reprocessing of resource B, delayed enqueue coalesces multiple triggers within a short time window:

```text
enqueue_after("ns/gateway-1", 100ms, chain):
  scheduled already contains "ns/gateway-1"?
    ├─ yes → skip (scheduled-level dedup)
    └─ no  → insert into scheduled → send DelayedItem to delay_tx
```

A background delay-loop task manages a `BinaryHeap<DelayedItem>` (min-heap ordered by `ready_at`):

```text
loop {
    if heap is empty → block waiting on delay_rx.recv()
    else {
        tokio::select! {
            sleep(next due time) => {
                pop all due items → check pending dedup → send to ready queue
            }
            delay_rx.recv() => {
                push the new delayed item into the heap
            }
        }
    }
}
```

Once a delayed item is due, it is removed from `scheduled`, checked against `pending` to avoid duplicates, inserted into `pending`, and sent to the ready queue.

## TriggerChain — Cascade Causality Tracking

TriggerChain is similar to HTTP's `X-Forwarded-For`; it records the resource path that caused a cascading requeue:

```rust
pub struct TriggerChain {
    pub sources: SmallVec<[TriggerSource; 4]>,  // stack-optimized; ≤4 hops have no heap allocation
}

pub struct TriggerSource {
    pub kind: &'static str,   // e.g. "HTTPRoute", "Gateway"
    pub key: String,           // e.g. "default/my-route"
}
```

### Building the Chain

After each ResourceProcessor finishes processing, before requeueing downstream resources it calls `chain.extend(kind, key)` to append its own info:

```text
Original event: Secret/default/tls-cert changed
  → triggers requeue of Gateway/default/gw-1, chain = [Secret/default/tls-cert]
    → triggers requeue of HTTPRoute/default/route-1, chain = [Secret/default/tls-cert → Gateway/default/gw-1]
```

### Cycle Detection

Before enqueue, `would_exceed_cycle_limit()` checks how many times the target (kind, key) appears in the chain:

```text
chain.occurrence_count(target_kind, target_key) >= max_trigger_cycles?
  ├─ yes → terminate the cascade, do not enqueue
  └─ no  → enqueue normally
```

`max_trigger_cycles` defaults to 5, `max_trigger_depth` defaults to 20 (total-depth safety net).

Example: if `HTTPRoute/default/route-1` has already appeared 5 times in the chain, the next attempt to requeue this resource is blocked, preventing an infinite loop.

## Worker Lifecycle

A worker is the async task that consumes the Workqueue. Each `ResourceController` has one worker:

```text
ResourceController event loop:
  Event::Applied(obj) → processor.process_init_item() (init phase)
  Event::InitDone     → processor.on_init_done()
                        worker_handle = spawn_worker(processor, ...)  // the worker is started here
  Event::Applied(obj) → processor.process_work_item() (runtime phase, driven by the worker)
```

**The worker starts only after `InitDone`**. During the init phase (LIST replay), events are processed synchronously by the event loop; once `InitDone` marks the LIST complete, the worker is spawned and starts consuming the Workqueue, processing incremental events asynchronously.

This design ensures that:

- During the init phase, the full data set is processed in order without intermediate states caused by concurrency
- During the runtime phase, the Workqueue decouples event reception from processing, supporting dedup and delayed (coalesced) requeue

## Metrics (WorkqueueMetrics)

| Metric | Type | Description |
|--------|------|-------------|
| `adds_total` | AtomicU64 | Total number of enqueue operations |
| `depth` | AtomicU64 | Current queue depth |
| `delayed_total` | AtomicU64 | Total number of delayed enqueues |
| `high_water_depth` | AtomicBool | Edge-trigger latch: true while depth is at/above the high-water mark (one log line per crossing) |

All metrics use `Ordering::Relaxed` atomic operations — fine for monitoring but not strictly consistent.

## Key Source Files

| File | Responsibility |
|------|----------------|
| `src/core/controller/conf_mgr/sync_runtime/workqueue.rs` | Full Workqueue implementation |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/processor.rs` | ResourceProcessor; uses Workqueue |
| `src/core/controller/conf_mgr/conf_center/kubernetes/resource_controller.rs` | K8s ResourceController, spawn_worker |
| `src/core/controller/conf_mgr/conf_center/file_system/resource_controller.rs` | File-system ResourceController, spawn_worker |
