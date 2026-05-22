---
name: conf-sync-compress-event-vec-per-key
description: Use when reviewing findings that the conf_sync CompressEvent per-key Vec<ResourceChange> is unbounded / causes OOM / degrades to an O(N) scan; confirmed-real but won't-fix design tradeoff.
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# conf_sync `CompressEvent` Per-Key `Vec<ResourceChange>` — DESIGN-TRADEOFF (won't-fix)

## Conclusion

`CompressEvent.events: HashMap<String, Vec<ResourceChange>>` (`src/core/gateway/conf_sync/cache_client/cache_data.rs`) accumulates one `Vec` element per event for a key inside the 100ms `BATCH_DEBOUNCE` window, and `process_compressed_events` only reads it via the binary `events.iter().any(|e| matches!(e, InitAdd | EventAdd))` scan. The per-key `Vec` and the `any()` scan are therefore `O(event_count)` where the consumer needs only `O(1)`. This is **confirmed-real but accepted won't-fix**: the cost is trivially bounded and lives on the control plane, not the data-plane request path.

Fix suggestions of the form "collapse `Vec<ResourceChange>` to a single `has_add: bool` flag" / "the per-key Vec is unbounded and risks OOM under a 1000-pod rolling update" / "eliminate the O(N) `any()` scan" are **recognized as a valid but not-accepted optimization** — the change is correct and behavior-preserving, but the churn is not justified at the current scale. Closed won't-fix on 2026-05-21.

## Core Rationale

**1. The "unbounded memory / OOM" framing is inaccurate.**

`ResourceChange` is a 6-variant `Copy` enum that fits in 1 byte (`src/core/common/conf_sync/traits.rs:5-19`). Even 1000 events on a single key is ~1 KB plus one `Vec` heap allocation — negligible. The `HashMap` key count is bounded by the number of distinct resource names, which is governed by the controller's event batch size. There is no realistic OOM surface.

**2. It is a control-plane path, not the request hot path.**

`add_event` is reached only from `apply_change` / the watch-stream loop (`event_dispatch.rs`), i.e. config sync on relist / watch events / rolling updates. The `any()` scan runs once per debounce window (`BATCH_DEBOUNCE = 100ms`) inside `process_compressed_events`, never per request. The real cost is a few wasted allocations + a short scan on an event-level path; it does not affect data-plane latency.

**3. The proposed fix is correct but pure churn at this scale.**

Collapsing to `has_add: bool` (OR each event's add-ness into the flag, read the flag directly) is exactly equivalent to the current `any()` check — both are order-independent — so it is behavior-preserving. But it touches the add/update/remove classification logic and its unit tests for a benefit that is unmeasurable at current event volumes. The project's standing rule is to defer such optimizations until profiling proves a real bottleneck (see [performance-review-methodology.md](../performance-review-methodology.md)).

## Fix Suggestions Not Accepted

- "Collapse the per-key `Vec` to `has_add: bool`" — correct and behavior-preserving, but unmeasurable benefit on a control-plane event-level path; not worth the classification-logic + test churn now.
- "Add a capacity cap / `with_capacity` to the per-key `Vec`" — caps a non-problem; the Vec is already bounded by the per-key event count within one 100ms window and the data is 1 byte/event.
- "Switch to a different container to avoid the O(N) scan" — same as above; the scan is on the debounced control-plane path, not measured as a hotspot.

## Re-evaluation Triggers

Re-open this decision only if:

- Profiling shows the `any()` scan or the per-event `Vec` allocation in `process_compressed_events` / `add_event` as a measurable hotspot on the conf-sync apply path.
- Per-key event volume within one debounce window grows far beyond the controller batch size, making the per-key `Vec` non-trivial memory.
- The classification logic starts needing more than the binary `has_add` property (e.g. the full ordered event sequence), which would change the storage calculus.

## Reference Cases

- F-012 (`tasks/issues/conf-sync-F012-event-compression-unbounded-vec.md`), Round B verdict: confirmed, OOM overstated, severity → Low; closed won't-fix 2026-05-21.
- Source: `src/core/gateway/conf_sync/cache_client/cache_data.rs` (`CompressEvent`, `add_event`, `process_compressed_events`).
- Enum size: `src/core/common/conf_sync/traits.rs:5-19` (`ResourceChange` 1-byte `Copy`).
- Methodology: [cpu-memory/performance-review-methodology.md](../performance-review-methodology.md).
