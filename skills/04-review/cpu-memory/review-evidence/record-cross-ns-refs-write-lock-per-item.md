---
name: record-cross-ns-refs-write-lock-per-item
description: Use when reviewing findings that flag record_cross_ns_refs looping add_ref with one write lock per backend_ref as a lock hotspot needing batch_update; this is a cold reconcile path.
knowledge_type: review-evidence
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# Route Handlers `record_cross_ns_refs` Loop `add_ref` is Not a Lock Hotspot

**False-positive scenario**: the audit flags `record_cross_ns_refs` (`resource_processor/handlers/{http,udp,grpc,tcp,tls}_route.rs:67-90` same pattern) for calling `manager.add_ref(...)` for every cross-namespace `backend_ref`, taking a write lock each time; suggests introducing `batch_update` for bulk-insert within a single lock.

**Reality**:

1. **Cold path — triggered by K8s resource watch event**: the call source is Route resource reconcile, event level (operator scale), not request level. The previous line in the same function `manager.clear_resource_refs(...)` itself takes a write lock once.
2. **Lock is `parking_lot::RwLock<RefMaps<V>>`** (`ref_manager.rs:170`), uncontended write ~25 ns; a single route typically has 1-10 backend_refs; pathological 100 → total lock overhead ~25 ns–2.5 µs. The path total is dominated by etcd RTT + resource parsing + status update; a few lock acquisitions are not measurable in totals.
3. **Change surface vs benefit**: to "save" you'd have to add `add_refs_batched` / `batch_update` methods to `BidirectionalRefManager`, or expose inner so the caller manages the lock — breaks encapsulation, expands API surface, requires changes in 5 handlers (http/udp/grpc/tcp/tls). Zero observable benefit on the cold path.
4. Same category as the existing "Small-object clones on Resource reconcile / config sync paths are not issues" — reconcile event level + small scale + cascading change surface.

**Verdict**: not a performance issue. Any "`record_cross_ns_refs` / similar reconcile handler with loop one-lock-per-item should be batched / done within a single lock" finding is closed per this entry, unless it is proved that (a) this path is reshaped to be triggered at request level, or (b) a single resource has ≥ thousand backend_refs and profile shows lock waits are significant.
