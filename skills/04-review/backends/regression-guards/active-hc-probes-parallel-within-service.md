---
name: active-hc-probes-parallel-within-service
description: Use when reviewing active health-check probe scheduling findings; covers concurrent per-service probes, serialized Coordinator completion handling, and the per-probe global semaphore.
status: fixed
finding_id: lb-06
closed: 2026-06-14
knowledge_type: regression-guards
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

**2. Parallelize only the probe I/O — serialize completion handling in the driver**

The slow part is network I/O. The loop runs probe futures concurrently and handles
their completions sequentially in the driver task as each future resolves. Current
production completion writes go to
`ServiceDiscoveryCoordinator::record_health_completion`, not
`HealthStatusStore::record_check`. The Coordinator validates policy generation,
Service membership, and endpoint identity under the endpoint completion lock, so
an old probe cannot update a removed endpoint or replacement policy.

`HealthStatusStore` no longer participates in routing-health publication. Its
Service marker is owned by serialized reconciliation rather than a probe cycle;
see [active health task status-marker ownership](active-health-task-status-ownership.md).
The historical unlocked `unhealthy_backends` concern belonged to the former
hash-keyed engine, removed under ARCH-022. Current threshold/reset/isolation tests
exercise Coordinator completion handling; the marker registry has no endpoint
health algorithm or publication API.

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
- "Write completions from detached per-backend tasks" — unnecessary and would
  complicate cancellation and generation ownership. Keep completion handling in
  the service task unless the Coordinator contract is deliberately redesigned.
- "Spawn a `tokio::task` per backend" — unnecessary; `FuturesUnordered` polled in the existing
  per-service task gives the same concurrency without per-backend task spawn/teardown.

## Re-evaluation Triggers

Re-open this decision only if:

- The loop reverts to a sequential per-backend `.await` (no `FuturesUnordered` / concurrent
  driving).
- completion handling moves into detached per-backend tasks or stops using the
  Coordinator's policy/membership/endpoint identity checks.
- A real cross-service probe-starvation report appears under the parallel model (would justify
  the per-service sub-cap rejected above).

## Reference Cases

- lb-06 (review finding, 2026-06-14)
- backends-F016 (`probe-semaphore-per-cycle.md`) — per-probe (not per-cycle) semaphore
  acquisition; this fix builds on it and keeps its semantics. F016 had listed within-service
  parallelism as out-of-scope for *that* finding; lb-06 is the follow-up that adopts it.
- Same module as backends-F001 (IPv6 probe URL), backends-F002 (task handle leak).
