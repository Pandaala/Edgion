---
name: active-hc-probes-parallel-within-service
description: Use when reviewing active health-check probe scheduling findings; covers the decision to probe all backends of one service concurrently (FuturesUnordered) instead of sequentially, while keeping record_check single-writer and the global probe semaphore per-probe.
status: fixed
finding_id: lb-06
closed: 2026-06-14
---

# lb-06 — Active health-check probes run in parallel within a service — FIXED

**Status:** ✅ fixed · 2026-06-14
**Severity:** Low — sequential per-service probing inflated the worst-case sweep to `B × timeout`, lagging unhealthy-backend detection on large services (>10–20 backends)

## Conclusion

`health_check_loop` (`edgion-gateway/src/backends/health/check/manager.rs`) now probes all
backends of a service **concurrently** within each interval via a `FuturesUnordered`, instead
of the previous strictly-sequential `for backend in backends` loop. The worst-case sweep time
is now `max(probe_i)` rather than `sum(probe_i)`, matching Envoy's per-host parallel active
health checking. Each probe future independently acquires a `PROBE_SEMAPHORE` permit for the
duration of its single probe and releases it immediately, so the global `MAX_CONCURRENT_PROBES`
cap and the per-probe acquire/release fairness semantics from backends-F016 are preserved.

Re-reports of the form "probes within a service are sequential", "a service with B backends
takes B × timeout per sweep", "MissedTickBehavior::Skip drops ticks during a long sweep so the
effective interval balloons", or "end-of-list backends have biased detection latency" are
**not accepted** — verify the current loop builds a `FuturesUnordered` of per-backend probe
futures (not a sequential `.await` per backend) before re-reporting.

## Core Rationale

**1. The defect was sweep latency, not correctness**

Probes were issued one at a time, each `.await`ing to completion (bounded by `timeout`, default
3 s) before the next started. For B backends the worst-case sweep was `B × (timeout + RTT)`; at
B = 20, timeout = 3 s that is up to 60 s — longer than the 10 s `interval`, so `MissedTickBehavior::Skip`
silently dropped the intervening ticks and compressed the effective probe frequency. Backends
late in the iteration order were always probed last (systematic detection-latency bias). Envoy
maintains a per-host connection + timer and probes every host in parallel, so its sweep is
`max(RTT_i)`; the fix brings Edgion to the same property.

**2. Parallelize only the probe I/O — keep `record_check` single-writer**

The slow part is the network probe; `record_check` is a cheap counter update. The fix runs the
probes concurrently but applies `record_check` **sequentially in the driver task** as each future
resolves (`while let Some((hash, addr, result)) = probes.next().await`). This is deliberate:
`HealthStatusStore::record_check` updates the per-service `unhealthy_backends` ArcSwap with a
load-clone-modify-store **outside** the per-service `states` Mutex (`status_store.rs:144-152`).
If `record_check` were called concurrently for different backends of the **same** service, that
RCU would become a lost-update race. Keeping the cycle's write side single-writer means the fix
**introduces no new concurrency hazard** while parallelizing the probe I/O, and avoids touching
`status_store.rs`.

Scope of the guarantee: this makes the *probe futures of one cycle* non-concurrent writers. It
does **not** newly serialize the pre-existing, change-unrelated reconcile-overlap window —
`upsert_task` spawns the new loop and only then `abort()`s the old one, so for a brief window two
loops can both write the same `ServiceHealthState` and race the unlocked `unhealthy_backends` RCU.
That window predates this fix (the old sequential loop had it too), is **self-healing** (each
cycle's `register_service` recomputes `unhealthy_backends` from the `states` map,
`status_store.rs:164-176`), and is *smaller* under parallel probing because cycles finish in
`max(probe_i)` instead of `sum(probe_i)`. The only production writers to `ServiceHealthState` are
this loop (`register_service` + `record_check`) and `unregister_service`; the request path only
reads via `is_healthy`.

**3. Global semaphore semantics unchanged (backends-F016 preserved)**

Each probe future acquires/releases one `PROBE_SEMAPHORE` permit around its single probe call,
exactly as backends-F016 established. The global cap (`MAX_CONCURRENT_PROBES = 50`) still bounds
total in-flight probes across all services. The only intentional change versus the F016 era is
the **load profile**: a single large service can now hold multiple permits at once (one per
in-flight probe) instead of one at a time. This is bounded — a service's probes drain within
roughly one `timeout` wave and release their permits — and is strictly closer to Envoy (which
has no per-cluster probe throttle at all). No per-service sub-cap was added: the F016
catastrophic case (one permit held for an entire `B × timeout` cycle, ~180 s) cannot recur
because every permit is released after a single ~timeout-bounded probe.

## Fix Suggestions Not Accepted

- "Add a per-service in-flight probe sub-cap to throttle large services" — not adopted: it
  re-introduces a throttle Envoy does not have and is unnecessary because per-probe permit
  release already bounds any single service's permit hold to one `timeout` wave. Re-open only
  if a real cross-service starvation report appears under the parallel model.
- "Apply `record_check` concurrently inside each probe future" — would expose the
  `unhealthy_backends` RCU lost-update race in `HealthStatusStore`; keep the write side
  single-writer in the driver task.
- "Spawn a `tokio::task` per backend" — unnecessary; `FuturesUnordered` polled in the existing
  per-service task gives the same concurrency without per-backend task spawn/teardown.

## Re-evaluation Triggers

Re-open this decision only if:

- The loop reverts to a sequential per-backend `.await` (no `FuturesUnordered` / concurrent
  driving).
- `record_check` is moved inside the concurrent probe futures, OR `HealthStatusStore` starts
  protecting the `unhealthy_backends` update under the per-service lock (then concurrent
  `record_check` would become safe and the single-writer constraint could be relaxed).
- A real cross-service probe-starvation report appears under the parallel model (would justify
  the per-service sub-cap rejected above).

## Reference Cases

- lb-06 (review finding, 2026-06-14)
- backends-F016 (`probe-semaphore-per-cycle.md`) — per-probe (not per-cycle) semaphore
  acquisition; this fix builds on it and keeps its semantics. F016 had listed within-service
  parallelism as out-of-scope for *that* finding; lb-06 is the follow-up that adopts it.
- Same module as backends-F001 (IPv6 probe URL), backends-F002 (task handle leak).
