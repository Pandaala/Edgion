---
module: load-balancing
title: Ejection duration is a fixed constant with no exponential backoff for repeat offenders
severity: medium
envoy_reference: Envoy outlier detection `base_ejection_time` + `max_ejection_time` with `num_times_ejected` multiplier
---

# Fixed Ejection Duration With No Backoff for Repeat Ejections

## Envoy's design (the proven baseline)

Envoy's outlier detection computes the **ejection duration** for a host as:

```
ejection_duration = min(
    base_ejection_time * num_times_ejected,
    max_ejection_time
)
```

`num_times_ejected` is a per-host counter that increments each time the host is
ejected and decays back toward zero while the host stays healthy. A host that is
repeatedly unhealthy will be ejected for progressively longer intervals (up to
`max_ejection_time`, default 300s), while a host that recovers and stays healthy
resets its multiplier. This backoff prevents the cluster from being bombarded with
repeated test-and-eject cycles against a flapping backend, which would amplify
traffic instability.

Reference: Envoy docs — Outlier Detection; `OutlierDetection.base_ejection_time`,
`OutlierDetection.max_ejection_time`, `OutlierDetection.max_ejection_time_jitter`.

## Edgion's current design

`CircuitBreakerConfig` (`src/core/gateway/backends/circuit_breaker/config.rs:8-15`)
has a single `ejection_seconds: u64` field (default 30 s). There is no
`num_times_ejected` counter, no `base_ejection_time` multiplier, and no
`max_ejection_time` cap.

`EndpointState` (`src/core/gateway/backends/circuit_breaker/store.rs:13-16`) tracks
only `consecutive_failures: u32` and `ejected_at: Option<Instant>`. When a backend
is un-ejected (either by `is_ejected`'s auto-recovery check at `store.rs:118-124`
or by `record_success` at `store.rs:192-197`), its entry is fully removed from
`svc.endpoints`. This means the ejection history is discarded on recovery: the next
ejection starts the clock again at the base `ejection_seconds`, regardless of how
many times this backend has been ejected before.

## The problem / gap

A chronically flapping backend (e.g. a pod with an OOM loop that restarts every
45 seconds) will be ejected for exactly 30 seconds on every cycle, then
re-admitted, serve a burst of failures, get ejected again, and so on. The gateway
absorbs the failure burst on every 30-second re-admission with no increasing
penalty. In Envoy the second ejection would last 60 s, the third 90 s, etc., up to
`max_ejection_time` — the flapping backend is progressively quarantined.

Without backoff:
- Each re-admission causes a predictable burst of 5 consecutive errors
  (`consecutive_errors` default) before re-ejection.
- The burst interval is `ejection_seconds` (30 s, fixed), so the gateway will
  experience periodic error spikes at a fixed cadence regardless of how bad the
  backend is.
- Operators get no signal that a backend is a repeat offender — all ejections look
  identical in metrics.

## Impact

- **Periodic error spikes** at fixed intervals for flapping backends with no
  damping; downstream clients see repeated 5xx bursts rather than increasingly rare
  re-admission attempts.
- **No operator visibility** into repeat-ejection patterns; `record_cb_ejection`
  fires identically for a first ejection and a tenth ejection.
- **Missing damping on the unhealthy tail**: backends that are permanently degraded
  (not fully failing, so active HC keeps them "healthy" but they generate errors)
  will cycle at full speed with no accommodation for the ejection history.

## Suggested direction

Add a per-endpoint `num_times_ejected: u32` counter to `EndpointState`. On ejection,
increment it and compute `actual_duration = min(base * num_times_ejected, max)`.
On successful recovery after a full ejection (i.e. the backend stayed healthy for
at least one probe cycle after un-ejection), decay `num_times_ejected` back toward
zero (or reset it outright). Persist the counter across ejection cycles by
retaining the `EndpointState` entry (with `consecutive_failures` reset but
`num_times_ejected` kept) when a backend recovers, rather than removing the entry
entirely. Expose `base_ejection_seconds` and `max_ejection_seconds` as additional
fields in `CircuitBreakerConfig`.
