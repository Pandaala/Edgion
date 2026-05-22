---
name: backends-F016-probe-semaphore-per-cycle
description: The health-check probe semaphore is now acquired per individual backend probe, not per probe cycle, so a service with many backends can no longer monopolize a permit for its whole cycle. Reject re-reports claiming the per-cycle permit starves unrelated services.
---

# backends-F016 — Probe semaphore acquired per probe, not per cycle

**Status:** ✅ fixed · 2026-05-21
**Severity:** Medium (BadLogic) — per-cycle permit could starve unrelated services' probe cycles in large clusters

## Conclusion

`health_check_loop` (`src/core/gateway/backends/health/check/manager.rs`) now acquires the
`PROBE_SEMAPHORE` permit **inside the `for backend in backends` loop**, scoped to a single probe
call and released (`drop(permit)`) immediately after the probe returns and before `record_check`.
The global cap (renamed `MAX_CONCURRENT_PROBES`, value 50) therefore bounds the number of
**in-flight individual probes** across all services, not the number of concurrent probe cycles.

Re-reports of the form "the probe semaphore is held for the entire multi-backend cycle", "a
service with N backends holds one permit for N × timeout", or "50 large services consume all
permits and starve other services' probe cycles" are **not accepted** — verify the current
`health_check_loop` acquires the permit per probe (not at the top of the `loop` body) before
re-reporting.

## What was wrong (before the fix)

The acquire was `let _permit = semaphore.acquire().await...` at the **top of the outer `loop`
body**, so the permit was held for the full duration of the sequential `for backend in backends`
loop. A service with 60 backends and a 3 s timeout could hold a single permit for up to ~180 s —
far longer than the default 10 s interval. If 50 such services entered their probe cycles at once
(startup jitter only spreads them by `interval/2`), all 50 permits were occupied for the entire
window and no other service could run its probe cycle. The semaphore's documented intent ("backpressure
without dropping or delaying individual tasks") was violated: it delayed whole cycles of unrelated
services rather than rate-limiting single probes.

## Why the fix is correct

A single service still probes its backends sequentially, so it never holds more than one permit at a
time — the same as before. The change is the **hold duration and interleaving**: with per-probe
acquisition the permit is released after each ~timeout-bounded probe, so a busy service yields the
permit between probes and Tokio's FIFO-fair `Semaphore` lets waiting services interleave instead of
blocking for a full cycle. There is no deadlock risk (each task holds at most one permit and releases
it before re-acquiring) and the per-probe acquire cost is negligible against network-I/O latency.
The non-IP-backend `continue` branch acquires no permit, so only real probe I/O consumes capacity.

## Fix Suggestions Not Accepted

- "Raise `MAX_CONCURRENT_PROBES` instead of changing the acquisition granularity" — masks the
  starvation only until the cluster grows again; the root cause is the per-cycle hold, not the cap.
- "Probe all backends of a service concurrently (spawn per backend)" — out of scope and changes the
  load profile; the finding is about cross-service fairness, which per-probe acquisition already fixes
  while keeping per-service probing sequential.

## Re-evaluation Triggers

Re-open only if:

- The acquire is moved back to the top of the `loop` body (per cycle) or widened to wrap the whole
  `for` loop.
- `MAX_CONCURRENT_PROBES` is repurposed to count cycles/tasks again rather than in-flight probes.

## Reference Cases

- backends-F016 (review finding, 2026-05-21)
- Same module as backends-F001 (IPv6 probe URL), backends-F002 (task handle leak).
