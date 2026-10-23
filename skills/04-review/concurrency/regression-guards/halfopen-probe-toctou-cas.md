---
name: halfopen-probe-toctou-cas
description: Review the passive-only outbound recovery timestamp CAS, current backoff interval, and limits of the single-winner argument.
knowledge_type: regression-guards
---

# Passive-only recovery timestamp CAS and zero-backoff scope

## Conclusion and scope

The current owner is `plugins/http/common/outbound/call.rs::should_halfopen_probe`.
It reads the live healthy flag, excludes active-health-managed clients, requires
passive backoff configuration, and uses a single timestamp compare_exchange to
claim a due window. A failed CAS returns false without retrying. The calling
health gate also reads healthy; these are live observations, not immutable
configuration values that can be cached as a performance shortcut.

For a positive current interval and a due timestamp strictly greater than the
observed baseline, competing callers with that same baseline cannot all win the
CAS. This is a conditional timestamp-arbitration property, not a coherent
snapshot of all HealthState atomics or a guarantee of one globally in-flight
probe. Another window can arrive while an earlier request remains pending.
Concurrent recovery/reset writers require their own analysis.

A zero current interval intentionally allows probing on every call. When now
and last are equal, compare_exchange(last, now) can succeed repeatedly. The
historical decision accepted this disabled-backoff behavior. A positive initial
interval alone does not establish a positive current interval: the next interval
is computed from the multiplier and maximum, and successful recovery resets it.
This page does not newly approve every possible multiplier/cap configuration.

## Current implementation boundary

HealthState stores healthy, passive_failures, last_halfopen and backoff_sec as
separate atomics. `record_passive_result` resets failures, healthy and backoff on
success; a threshold-crossing failure establishes the configured interval and
process-monotonic baseline. It does not turn these fields into one transaction.

`process_elapsed_secs` uses a process-wide OnceLock<Instant> origin and
saturating elapsed duration. Zero elapsed seconds is a real timestamp, not an
uninitialized sentinel. The current due predicate is:

```rust
interval == 0 || now >= last.saturating_add(interval)
```

The gate loads last with Acquire, interval with Relaxed, and performs one
compare_exchange with AcqRel success and Relaxed failure ordering. A winner
stores the multiplied/capped next interval with Relaxed ordering. At the
synthetic saturated boundary now == last == u64::MAX, the due predicate alone
does not imply a timestamp-changing CAS; exclude that case from the conditional
single-winner argument. Do not replace the saturating arithmetic with unchecked
addition or claim that changing the atomics independently creates a snapshot.

## Historical evidence and limits

F-B02 replaced a load/check/plain-store admission sequence with a CAS on
2026-05-25. The former `link_sys/providers/webhook/health.rs` implementation was
removed on 2026-07-08; current production uses the shared outbound call loop.
The historical contention-test name is not evidence of a current test run.
The earlier blanket assertion that every other caller must lose omitted the
zero-interval and timestamp-changing qualifications above.

A CAS retry loop would change admission behavior: a loser currently returns
Unhealthy rather than retrying against a newly observed baseline. Removing a
live read or replacing the gate with a blocking lock is not an established
performance improvement. A new admission/lifetime contract requires independent
analysis rather than relying on the mere presence of compare_exchange.

## Verification and re-evaluation

Current `call_test.rs::halfopen_window_due_preserves_zero_interval_and_saturates_boundaries`
covers zero interval, exact boundary and saturating arithmetic. Existing retry
and scripted-transport tests cover fixed/exponential delays, jitter bounds,
Retry-After behavior, deadlines and terminal outcomes. A fresh serial shared
outbound selection passed 123 tests during this documentation audit. No new
contention test, live transport test, benchmark or proof of every concurrent
health-state transition was produced.

Re-evaluate when baseline writers, current-interval semantics, clock units,
recovery/reset ordering, active-health interaction or probe lifetime requirements
change. Verify source and tests before applying this historical conclusion.

## References

- F-B02 / link-sys-FB02: original passive recovery TOCTOU correction.
- `edgion-gateway/src/plugins/http/common/outbound/{call,client}.rs`: current owners.
- [Shared outbound unit-test boundary](../../testing/regression-guards/shared-outbound-call-unit-tests-are-network-free.md).
- [Sliding-window CAS](sliding-window-check-then-increment.md): a different CAS
  retry policy; its acquisition loop is not permission to retry this probe gate.
