---
name: retry-budget-epoch-buckets
description: RetryBudget monotonic interpolation, atomic epoch publication, admission slack, and platform bounds.
status: shipped
---

# Epoch-tagged retry-budget buckets

## What we run today

`RetryBudget` in
`edgion-gateway/src/backends/policy/backend_traffic/retry_budget.rs` resolves
GEP-2257 durations once, retaining integral milliseconds. Each operation samples
`Instant::now().saturating_duration_since(origin)` once. For each interval:

- `epoch = elapsed_ms / interval_ms + 1`; zero is reserved for an unused slot.
- `previous_weight = 1 - (elapsed_ms % interval_ms) / interval_ms`.
- `estimate = current + previous * previous_weight`.
- Retry eligibility is `retries < ceil(percent / 100 * attempts)` OR
  `floor_retries < floor_count`; every admitted retry updates both counters.

Each of two alternating slots is one `portable_atomic::AtomicU128` containing
48 epoch bits and two 40-bit counts (attempts, retries). The floor uses the same
window implementation with attempts unused. A CAS initializes an older slot and
increments the selected count together. It never moves counters between epochs.

## Contract

Storage is constant per budget; operations allocate nothing and emit no tracing.
The noncontended path uses a fixed number of atomic operations. CAS and snapshot
validation retry under contention, so it is lock-free, not wait-free.

All slot accesses use SeqCst. A snapshot loads current, previous, then current
again. Equal current words prove both words coexisted at the middle load:
counts are monotonic within each epoch and epoch identifiers never wrap, so
there is no ABA. Each slot's numerator and denominator are read atomically.
Unexpected older tags contribute zero; tags newer than the sampled history
reject the decision. Two or more elapsed intervals cannot resurrect old traffic.

An operation belongs to the epoch sampled at entry. A delayed attempt may still
update that epoch while its slot survives. It cannot overwrite a newer slot.
A retry whose required snapshot or budget commit was superseded is denied.
After budget admission commits, an expired floor increment is discarded without
rollback; it cannot charge a newer floor epoch. A surviving floor slot still
receives the increment in the sampled epoch.

The check and increments are intentionally separate: N concurrent deciders may
over-admit by up to N-1. This is the accepted coarse anti-storm soft cap, not an
exact rate limiter. Epoch publication adds no lost increment to retained history.

`2^40 - 1` is a sticky count saturation sentinel; any retained saturated bucket
denies decisions until it ages out. It is approximately 1.1 trillion events per
bucket (over 300 million/second for a one-hour interval). Counts and 48-bit epoch
identifiers never wrap. Epoch exhaustion or elapsed-time conversion overflow
denies retries for the instance. Zero/invalid intervals from nonvalidated callers
retain a defensive positive floor; operator intervals are validated separately.

Gateway startup checks `AtomicU128::is_lock_free()` before serving. amd64 needs
CMPXCHG16B, including inside VMs; arm64 uses the dependency's native implementation.
The check applies before hot-reloaded policies can create budgets. No lock-based
fallback is permitted in a serving Gateway.

## Why this algorithm

The old independent atomics published the timestamp before swapping counts.
A paused rotator could move a new-window increment to previous, while observers
could mix numerator and denominator phases. Tagged words eliminate that transition
without a hot-path allocation, a writer-owned rotation flag, or a blocking lock.

## Prior art comparison

| Implementation / alternative | Decision |
|---|---|
| Repository `SlidingWindowCounter::try_acquire` in `edgion-gateway/src/link_sys/providers/webhook/runtime.rs` | Retain its two-window interpolation math; do not copy timestamp-first rotation or treat its single-count admission CAS as proof of epoch coherence. |
| `portable-atomic` 1.14.0 native atomic128 implementations | Reuse the dependency's native atomic operations and runtime capability detection; do not implement platform assembly in Edgion. |
| Mutex or a writer-owned seqlock | Reject: a paused writer can block every request for that backend. |
| Per-operation ArcSwap snapshots | Reject: allocating replacement snapshots on every attempt violates this task's hot-path allocation constraint. |

## Known limitations

The soft cap, stale-decision denial, saturation policy, and minimum CPU capability
were explicitly accepted for this fix. This does not establish a workspace-wide
mandate to replace all counters with AtomicU128. Native amd64 behavior still
requires validation on amd64; arm64 results are not cross-platform benchmarks.
Heavy contention on one budget increases both CAS retries and snapshot
revalidation. Fewer clock reads do not imply a throughput improvement under that
load; a workload where every attempt immediately requests a retry stresses this
shared state much more than ordinary low-retry traffic.

## Replacement criteria

Revisit if supported CPU platforms change, atomic contention becomes a measured
bottleneck, or the product requires exact admission rather than a soft cap.
Any replacement must preserve epoch identity, bounded history, one time sample,
allocation-free accounting, and request-stable budget Arcs.

## References

- [Retry-budget regression boundary](../04-review/backends/regression-guards/retry-budget-epoch-coherence.md)
- [Time conventions](../03-coding/03-time-conventions.md)
- [portable-atomic atomic128 source](https://github.com/taiki-e/portable-atomic/tree/v1.14.0/src/imp/atomic128)
- [portable-atomic platform/fallback contract](https://github.com/taiki-e/portable-atomic/blob/v1.14.0/README.md)
