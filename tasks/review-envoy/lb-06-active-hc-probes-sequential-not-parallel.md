---
module: load-balancing
title: Active health check probes within a service are sequential, not parallel — large backend sets cause probe lag
severity: low
envoy_reference: Envoy health checker probes all hosts in parallel per cluster, each with its own independent timer
---

# Active Health Check Probes Are Sequential Within a Service Interval

## Envoy's design (the proven baseline)

Envoy's active health checker maintains a **per-host health check connection and timer**.
Each host is probed independently and in parallel — a cluster with 100 backends fires
100 probes concurrently (subject to I/O), each on its own jittered schedule. A slow
or timing-out probe on one backend does not delay probes to other backends. The
health check interval is therefore effectively per-host, not per-cluster-sweep.

Reference: Envoy health checking; `HttpHealthChecker` — one connection per endpoint,
independent timers.

## Edgion's current design

`health_check_loop` (`edgion-gateway/src/backends/health/check/manager.rs:110-245`)
iterates the backend list **sequentially** within a single `tokio::task`:

```rust
for backend in backends {       // manager.rs:156
    let permit = semaphore.acquire().await...;   // manager.rs:179
    let result = match config.r#type {
        HealthCheckType::Http => probe_http_with_client(...).await,
        // ...
    };
    drop(permit);
    health_store.record_check(...);
}
```

Each probe `await`s to completion (subject to `timeout`, default 3 s) before the
next probe starts. The global semaphore (`MAX_CONCURRENT_PROBES = 50`,
`manager.rs:24`) prevents a single large service from monopolizing all probe slots
across services, but within a single service the probes are strictly sequential:
backend N+1 is not probed until backend N's probe completes or times out.

## The problem / gap

For a service with B backends, the worst-case probe sweep time is:
```
sweep_time = B × (probe_timeout + probe_network_RTT)
```

With B = 20 backends, timeout = 3 s: sweep_time up to 60 s. The `ticker` interval
is 10 s (default). Since `MissedTickBehavior::Skip` is set (`manager.rs:145`),
ticks missed during a long sweep are silently dropped — a 60 s sweep effectively
reduces the probe frequency to once per 60 s, not once per 10 s.

More critically, if the first backend in the list times out (3 s), the third
backend does not get probed until second 6 — 6 seconds into the 10 s interval.
A backend that goes unhealthy at second 7 will not be detected until the next
sweep starts, which could be 60+ seconds later on a large degraded cluster.

With Envoy's per-host parallel approach, all 20 backends are probed concurrently
and the sweep completes in `max(RTT_i + processing)` rather than `sum(RTT_i)`.

The semaphore does correctly prevent monopolization across services (`manager.rs:178-226`),
but does not parallelize probes within a service.

## Impact

- **Degraded health check responsiveness** for services with many backends (>10).
  Unhealthy backends may go undetected for multiple probe intervals.
- **Interval compression under load**: if one backend's probe is slow or the
  network is saturated, subsequent backends in the same sweep are delayed.
- **Skewed unhealthy detection time**: backends at the end of the iteration order
  are always probed last, creating a systematic bias in detection latency.
- This is a **low severity** finding because the semaphore prevents cross-service
  starvation, and services with many backends are relatively uncommon in small
  clusters. However it becomes significant at scale (>20 backends/service).

## Suggested direction

Spawn one probe task per backend within the service's health-check loop, using
`tokio::join_all` or `FuturesUnordered`. Each per-backend task acquires a semaphore
permit for the duration of its probe. This keeps the global `MAX_CONCURRENT_PROBES`
cap while parallelizing within a service. The per-service task structure remains
unchanged (one `JoinHandle` per service in `HealthCheckManager`); only the inner
loop becomes concurrent. The `record_check` call must remain on the shared
`health_store` (which is already `Arc` + `Mutex`-protected per-service, safe for
concurrent writes).
