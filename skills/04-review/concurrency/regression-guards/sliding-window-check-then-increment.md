---
name: sliding-window-check-then-increment
description: SlidingWindowCounter admission CAS closes the fixed-epoch check/increment gap; it does not establish coherent window rotation.
knowledge_type: regression-guards
---

# Webhook admission CAS: fixed-epoch correction and rotation limits

## Conclusion and scope

The historical `link-sys-F006` fix (2026-05-25, Low severity) replaced the
separate admission check and `current.fetch_add(1)` in
`SlidingWindowCounter::try_acquire` with a `compare_exchange_weak` retry loop.
Current source in `edgion-gateway/src/link_sys/providers/webhook/runtime.rs`
retains that loop. Within a fixed epoch it closes the original check/increment
gap; it does not prove correctness across rotation.

The historical assertion that timestamp-first rotation was "already correct"
was too broad. The current implementation publishes `window_start` before
`current.swap(0)` and `previous.store`. A paused rotator can move a new-window
increment into previous, and a reader caches `prev`/`ratio` outside the CAS loop.
This remains a real unresolved defect, tracked in
[webhook-sliding-window-epoch-coherence](../../../../tasks/todo/webhook-sliding-window-epoch-coherence.md).
The retry-budget fix does not modify this Webhook limiter.

## Rationale and evidence

The current fixed-epoch admission loop is:

```rust
let prev = self.previous.load(Ordering::Acquire) as f64;
loop {
    let curr = self.current.load(Ordering::Acquire);
    let estimated = prev * (1.0 - ratio) + curr as f64;
    if estimated >= self.limit as f64 {
        return false;
    }
    if self
        .current
        .compare_exchange_weak(curr, curr + 1, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        return true;
    }
}
```

A competing increment invalidates the CAS and forces a fresh current count.
However, a rotation is not guaranteed to fail that CAS: the count can return to
the same value (ABA), while the cached previous count and ratio describe another
epoch. The historical decision to leave these values outside the loop was a
scope limit of the original fix, not evidence that they remain coherent.

The original fix retained the pre-increment `>=` comparison and reported that
`allows_within_limit`, `rejects_over_limit`, and `zero_limit` passed unchanged.
Those are historical results, not fresh verification of epoch races.

## Alternatives and limits

Do not replace admission with optimistic `fetch_add` followed by rejection
`fetch_sub`: if a rotation resets the counter between them, rollback can
underflow into `u64::MAX`. The current CAS loop avoids that rollback failure;
it does not establish that all rotation races are closed.

The old recommendation against a Mutex concerned the small fixed-epoch
admission correction. It is not permission to ignore the remaining epoch
publication problem or to borrow RetryBudget's accepted soft-cap semantics for
a Webhook rate limiter.

## Verification and re-evaluation

Current source inspection for `retry-budget-generation-and-window-coherence`
confirmed the CAS loop and the timestamp-first rotation sequence. No new Webhook
runtime tests or acceptance of its remaining defect are claimed here.

Re-evaluate if the check/increment gap or rollback is reintroduced, or when
epoch publication, cached estimates, or multi-window expiry is assessed. The
epoch-boundary trigger fired in this investigation; the linked task stays open.

## Resolution references

- `link-sys-F006` — historical fixed-epoch correction, 2026-05-25.
- `retry-budget-generation-and-window-coherence` — narrowed the knowledge claim
  using current code; Webhook rotation remains separately tracked.
- [Shared-store atomic mutation](link-sys-runtime-insert-remove-rcu.md) is a
  related mechanism, not proof of this counter's rotation correctness.
