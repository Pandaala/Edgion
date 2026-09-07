---
name: sliding-window-check-then-increment
description: SlidingWindowCounter::try_acquire (webhook rate limiter) now decides and increments atomically via a compare_exchange retry loop, so concurrent webhook callers cannot over-admit past the window limit. Reject re-reports claiming the check-then-fetch_add gap allows bursts over the limit.
knowledge_type: regression-guards
---

# link-sys F-006 — `SlidingWindowCounter::try_acquire` decide+increment is now atomic via CAS retry

**Status:** ✅ fixed · 2026-05-25
**Severity:** Low (BadLogic) — concurrent webhook callers could admit a few requests over the configured window limit

## Conclusion

`SlidingWindowCounter::try_acquire`
(`edgion-gateway/src/link_sys/providers/webhook/runtime.rs`) previously did the
rate-limit decision as two non-atomic atomic ops: it loaded `current`, computed
`estimated` and compared it to `limit`, then — in a separate step — called
`self.current.fetch_add(1)`. Between the check and the increment there was no
serialization, so N concurrent async webhook-call tasks could all read the same
sub-limit `estimated`, all pass the check, and all increment, admitting up to N
requests when only 1 permit remained (over-admission bounded by the number of
concurrent callers).

The decision and the increment are now fused into a single
`compare_exchange_weak` retry loop:

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

The increment only commits when `current` is unchanged since the read; on
contention the CAS fails and the loop re-reads, so concurrent callers serialize
on the counter and the limit holds exactly. The window-rotation CAS (the
`compare_exchange` on `window_start` + `swap(0)` at the top of the function) was
already correct and is unchanged. Re-reports of the form "the check-then-`fetch_add`
gap lets concurrent webhook calls burst over the rate limit / TOCTOU between the
`estimated >= limit` check and the increment" are **not accepted** — verify the
current `try_acquire` uses the `compare_exchange_weak` loop before re-reporting.

## Core Rationale

**1. Single-threaded semantics are byte-identical.** The fix keeps the original
`>=` comparison against the *pre-increment* `curr`, so each existing unit test
(`allows_within_limit`, `rejects_over_limit`, `zero_limit`) passes unchanged: the
loop grants exactly `limit` permits per window, then rejects.

**2. CAS retry, not optimistic `fetch_add` + rollback.** The issue file's primary
suggestion (optimistic `fetch_add` then conditional `fetch_sub` on rejection) was
**not adopted**: if the window rotates (`swap(0)`) between the speculative
`fetch_add` and the rollback `fetch_sub`, the `fetch_sub` underflows the freshly
zeroed `current` to `u64::MAX`, rejecting every request for the rest of that
window — a worse failure mode than the mild over-admission it fixes. The CAS loop
has no rollback: a concurrent rotation between the `load` and the
`compare_exchange` simply fails the CAS and the loop retries with the fresh value,
so there is no underflow window.

**3. `prev`/`ratio` stay a pre-loop snapshot.** Only the decision+increment on
`current` was racy, so only that is wrapped in the loop. `prev` and `ratio` are
read once before the loop exactly as the original code did — minimal scope, no
semantic change to the sliding-window interpolation.

**4. Same atomic-mutation principle as the sibling link-sys / backends fixes.**
This is the "make the multi-step read-modify-write on a shared atomic/structure
atomic via compare-and-swap with retry" class, identical in spirit to
[link-sys-runtime-insert-remove-rcu](link-sys-runtime-insert-remove-rcu.md)
(`ArcSwap::rcu`) and backends-F002 / F004 / F005.

## Fix Suggestions Not Accepted

- "Optimistic `fetch_add` then `fetch_sub` rollback on rejection" — introduces a
  `u64` underflow at the window-rotation boundary (see Core Rationale 2). The CAS
  loop is the rollback-free correct form.
- "Wrap `try_acquire` in a `Mutex`" — unnecessary; the counter is a handful of
  atomics and the CAS loop already serializes writers without blocking reads.

## Re-evaluation Triggers

Re-open only if:

- `try_acquire` reverts to a separate check-then-`fetch_add` (reopening the
  over-admission window), or
- a new optimistic-`fetch_add`+`fetch_sub` variant is introduced (reopening the
  rotation-underflow window).

## Reference Cases

- link-sys-F006 (review finding, `tasks/link/`, 2026-05-25)
- [link-sys-runtime-insert-remove-rcu](link-sys-runtime-insert-remove-rcu.md) —
  sibling link-sys atomic-mutation fix (`rcu`).
- backends-F002 / F004 / F005 — same atomic-CAS-with-retry principle on
  config-sync–driven shared state.
- `edgion-gateway/src/link_sys/providers/webhook/runtime.rs` — `SlidingWindowCounter`
  and its only production caller `call.rs::webhook_call` (rate-limit gate).

## Resolution references

- `link-sys-F006` — 2026-05-25.
