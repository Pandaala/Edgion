---
name: control-plane-status-write-counter-pre-caching
description: Use when reviewing findings that suggest pre-caching Counter handles for record_status_write helpers on the controller plane to avoid per-call allocations.
knowledge_type: intentional-design
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# Control-Plane Status-Write Counter Handle Pre-Caching is Not Worth It

**False-positive scenario**: criticizing `record_status_write` / `record_status_write_error` / `record_status_write_skipped` in `edgion-controller/src/conf_mgr/sync_runtime/metrics.rs` for calling the `counter!` macro per invocation (two `String` allocations + registry hash lookup per call). Suggested fix is to mirror the sibling `ResourceMetrics` struct and pre-cache `Counter` handles per `(kind, backend, result)`.

**Reality**:
- These helpers are **controller-plane**, fired on reconcile events that produce a status change — not per request. Even under mass-reconcile the ceiling is ~10–100/sec, dwarfed by the K8s API PATCH (ms-scale) that immediately follows.
- The per-call cost is sub-microsecond (two short strings ≤ ~12 B + one registry hash); the surrounding write is 3–4 orders of magnitude slower.
- The existing `ResourceMetrics` pre-caches *gauges* per kind because gauges are written every init / resync and need a stable handle anyway; status-write counters are event-driven and don't share that motivation.
- Implementing the suggested fix is multi-file (new struct in `metrics.rs` + wire through 4 call sites in `processor.rs`, `conf_center/file_system/resource_controller.rs`, `conf_center/etcd/resource_controller.rs`, `conf_center/kubernetes/resource_controller.rs`). The kubernetes path is a free function with a runtime-built `kind_str` (from `K::kind(&dt).to_string()`), so caching there would require a global `LazyLock<DashMap<...>>` keyed by 3-tuple, adding code surface for no measurable benefit.

**Decision criteria**:
- "`record_status_write*` allocates two `String`s per call" → expected; the `metrics::counter!` API requires `Cow<'static, str>`, the alloc is the minimal form (same logic as the `.to_string()` entry above).
- "Should pre-cache `Counter` handles like `ResourceMetrics` does for gauges" → asymmetry is intentional; gauges and event-driven counters have different access patterns.

**Verdict**: not a performance issue on the controller plane; accepted tradeoff; fix suggestions are not accepted unless paired with a profiler-confirmed hot-spot.
