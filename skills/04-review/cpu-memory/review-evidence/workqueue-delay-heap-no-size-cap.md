---
name: workqueue-delay-heap-no-size-cap
description: Use when reviewing findings that flag the workqueue BinaryHeap<DelayedItem> for lacking an explicit size cap or LRU eviction policy; dedup by key and per-Kind bounds prevent unbounded growth.
knowledge_type: review-evidence
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# Workqueue Delay Heap Without Explicit Size Cap is Not an Issue

**False-positive scenario**: the `BinaryHeap<DelayedItem>` in `edgion-controller/src/conf_mgr/sync_runtime/workqueue.rs:330-384`'s `spawn_delay_loop` has no explicit capacity / size check; flagged as "unbounded growth can be abused / memory amplification risk"; suggests adding `max_heap_size` rejection or eviction policy.

**Reality**:

1. **Dedup by key before enqueuing**: `enqueue_after` (workqueue.rs:531-539) first calls `self.scheduled.insert(key)`; if already in the `scheduled` `DashSet`, returns false and does not send to `delay_tx`. The same key is in the heap at most once, until promote (workqueue.rs:357 `scheduled.remove`) re-permits scheduling.

2. **Each Workqueue serves only one resource type** (`workqueue.rs:250` "Each resource type should have its own workqueue instance"); heap size is naturally bounded:
   ```
   |heap| ≤ |unique keys scheduled-but-not-due under that Kind| ≤ total objects of that Kind in the cluster
   ```
   This upper bound is constrained by K8s apiserver/etcd themselves, typically thousands.

3. **delay_tx channel itself is bounded**: `mpsc::channel(config.capacity)` (default 5000, workqueue.rs:191, 295) — even if the heap could theoretically hold more, sender-side `delay_tx.send(...).await` will first block with backpressure.

4. **Pressure alarm exists**: workqueue.rs:541-551 logs `warn` when `pending + scheduled ≥ 80% capacity`; ops side is observable.

5. **Trigger chain has cycle/depth caps**: `max_trigger_cycles=5`, `max_trigger_depth=20` (workqueue.rs:193-194) block runaway cascade amplification; `promote_delayed_item` discards directly when limit is reached.

6. **The only scenario with abnormal heap growth is a resource type really having millions of objects in the cluster** — but in that case, informer/cache would have blown up first; adding a separate cap to the heap does not solve the root cause.

**Verdict**: not a memory/performance issue. Any "workqueue delay heap should add max_size / capacity check / LRU eviction" finding is closed per this entry, unless it is proved that (a) `scheduled` DashSet dedup is broken (per-case bug; fix the dedup rather than add a cap to the heap), or (b) profile measurement shows that under steady state heap size is significantly out of sync with the resource object count of that Kind.
