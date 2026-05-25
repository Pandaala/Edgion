---
name: link-sys-FB02-halfopen-probe-toctou-cas
description: should_halfopen_probe (webhook passive-only circuit recovery) now gates the probe with a compare_exchange on last_halfopen, so concurrent callers admit exactly one half-open probe per backoff window. Reject re-reports claiming the load-check-store gap lets multiple simultaneous probes through.
---

# link-sys F-B02 — `should_halfopen_probe` admits exactly one probe per window via CAS

**Status:** ✅ fixed · 2026-05-25
**Severity:** Low (BadLogic) — concurrent webhook callers could each pass the half-open gate and flood a recovering backend with simultaneous probes

## Conclusion

`should_halfopen_probe`
(`src/core/gateway/link_sys/providers/webhook/health.rs`) gates circuit-breaker
recovery for passive-only webhook health checking. It previously decided as a
non-atomic load → check → store on `last_halfopen`: it loaded `last`, tested
`interval == 0 || now >= last + interval`, then — in a separate step — did a plain
`last_halfopen.store(now)`. Between the load and the store there was no
serialization, so N concurrent async webhook-call tasks could all read the same
`last`, all satisfy the condition, and all return `true`, admitting up to N
simultaneous half-open probes when the window should release exactly one
(over-admission bounded by the number of concurrent callers).

The decision and the timestamp write are now fused with a `compare_exchange`:

```rust
let last = runtime.last_halfopen.load(Ordering::Acquire);
let interval = runtime.backoff_sec.load(Ordering::Relaxed);
if interval == 0 || now >= last + interval {
    if runtime
        .last_halfopen
        .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
    {
        let next_backoff = ((interval as f64) * backoff.multiplier) as u64;
        runtime.backoff_sec.store(next_backoff.min(backoff.max_sec), Ordering::Relaxed);
        return true;
    }
}
false
```

Only the caller whose `last_halfopen` is still `last` wins the swap and sends the
probe (and is the only one to advance `backoff_sec`); every other concurrent
caller fails the CAS and falls through to `false`. Re-reports of the form "the
load-check-store on `last_halfopen` is not atomic / two callers can both pass the
gate / thundering-herd of half-open probes at the backoff boundary" are **not
accepted** — verify the current `should_halfopen_probe` uses the
`compare_exchange` gate before re-reporting.

## Core Rationale

**1. No retry loop — exactly one probe is the goal.** Unlike F-006's rate-limit
counter (where a CAS loser must retry because it still needs a permit), a
half-open probe loser must NOT retry: the window has already released its single
probe to the winner. So the loser returns `false` (caller gets
`WebhookCallError::Unhealthy`) and waits for the next backoff boundary.

**2. Single-caller semantics unchanged.** With one caller the CAS always succeeds
(value is unchanged since the load), so the existing single-threaded tests
(`test_passive_health_check_marks_unhealthy`,
`test_passive_health_check_resets_on_success`) pass byte-identically.

**3. `interval == 0` degenerate config is acceptable.** When `backoff_sec == 0`
(operator set `initial_sec == 0`, i.e. backoff effectively disabled) and
`now == last`, the CAS `compare_exchange(last, now)` is a no-op that can succeed
for more than one caller. This is acceptable: that config means "probe on every
call", so admitting multiple probes is the configured intent, not a regression.
Under any real backoff (`initial_sec > 0`) the gate opens only when
`now >= last + interval` ⇒ `now > last`, so the CAS strictly distinguishes the
winner.

**4. Same atomic-mutation principle as the sibling link-sys / backends fixes.**
This is the "make the multi-step read-modify-write on a shared atomic atomic via
compare-and-swap" class, identical in spirit to
[link-sys-F006-sliding-window-check-then-increment](link-sys-F006-sliding-window-check-then-increment.md)
(`compare_exchange_weak` retry),
[link-sys-runtime-insert-remove-rcu](link-sys-runtime-insert-remove-rcu.md)
(`ArcSwap::rcu`), and backends-F002 / F004 / F005.

## Fix Suggestions Not Accepted

- "Wrap `should_halfopen_probe` in a `Mutex`" — unnecessary; the gate is a couple
  of atomics and a single CAS already serializes the writers without blocking.
- "Add a CAS retry loop like F-006" — wrong semantics here; a loser must not
  retry-and-probe (that would re-introduce the multi-probe behavior). Single-shot
  CAS is correct.

## Re-evaluation Triggers

Re-open only if:

- `should_halfopen_probe` reverts to a separate load-check-then-`store` on
  `last_halfopen` (reopening the multi-probe window), or
- a new caller advances `last_halfopen` outside the CAS, breaking the
  one-probe-per-window invariant.

## Reference Cases

- link-sys F-B02 (review finding, `tasks/link/`, 2026-05-25)
- [link-sys-F006-sliding-window-check-then-increment](link-sys-F006-sliding-window-check-then-increment.md)
  — structurally identical sibling (rate-limit counter CAS), fixed the same day.
- backends-F002 / F004 / F005 — same atomic-CAS principle on config-sync–driven
  shared state.
- `src/core/gateway/link_sys/providers/webhook/health.rs` —
  `should_halfopen_probe` and its only production caller `call.rs::webhook_call`
  (health gate). Concurrency regression test
  `test_halfopen_probe_admits_exactly_one_under_contention`.
