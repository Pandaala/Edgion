# Circuit Breaker

> **🔌 Edgion extension**
>
> This feature enables passive ejection for backends via the `EdgionBackendTrafficPolicy` custom resource's `outlierDetection` section, and is an Edgion extension capability.

## Overview

The circuit breaker (outlier detection) tracks consecutive failures for each backend endpoint on the gateway side. When the threshold is reached, the endpoint is automatically ejected; after the cooldown period expires, it automatically recovers.
This feature is used to prevent cascading failures and protect upstream services, complementing active health checks.

**Failure events** are split into three independent counters: consecutive **application** 5xx (any status ≥500 that is not 502/503/504), consecutive **upstream-origin gateway** failures (backend returned 502/503/504), and consecutive **local-origin** failures (connect failure/timeout, TLS-handshake timeout, transport reset — the gateway never got a usable HTTP response). Any one counter reaching its threshold ejects the endpoint.
**Success events** (non-5xx responses) reset the consecutive failure counts.

Outlier detection is configured via the `spec.outlierDetection` section of an `EdgionBackendTrafficPolicy` resource attached to the target `Service`. `EdgionBackendTrafficPolicy` also carries [active health checks](./health-check.md) and the [load balancing algorithm](../lb-algorithms.md) for the same Service; see the [full schema reference](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/20-edgion-backend-traffic-policy.md) for all three sections together.

## Quick Start

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: my-backend-policy
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: my-backend
  outlierDetection:
    consecutiveErrors: 5
    ejectionTime: 30s
    maxEjectionPercent: 50
---
apiVersion: v1
kind: Service
metadata:
  name: my-backend
  namespace: production
spec:
  ports:
    - port: 8080
      targetPort: 8080
```

## Configuration Reference

`spec.outlierDetection`:

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `consecutiveErrors` | Integer | `5` | Consecutive application-5xx failures (any ≥500 that is NOT 502/503/504) before ejection. Minimum `1`. |
| `consecutiveGatewayErrors` | Integer | defaults to `consecutiveErrors` | Consecutive upstream-origin gateway failures (backend returned 502/503/504) before ejection. |
| `consecutiveLocalOriginFailures` | Integer | defaults to the resolved `consecutiveGatewayErrors` | Consecutive local-origin failures (connect/TLS-handshake/transport-reset/proxy errors) before ejection. |
| `ejectionTime` | Duration | `30s` | Base ejection duration (GEP-2257 duration, multiplied by the endpoint's ejection count for backoff). Minimum `1s`. |
| `maxEjectionTime` | Duration | `max("5m", ejectionTime)` | Upper bound on the backoff-scaled ejection duration. |
| `maxEjectionPercent` | Integer | `50` | Maximum percent of a Service's endpoints that may be ejected at once. `0` means no limit; values above `100` are rejected. |

A spec that fails validation (for example `consecutiveErrors: 0`, `ejectionTime: 500ms` (sub-second durations are rejected), or `maxEjectionPercent: 150`) makes the whole `EdgionBackendTrafficPolicy` `Accepted=False`; following the lenient-admission pattern the resource still syncs, and the gateway skips the invalid spec, leaving outlier detection disabled for the target Service(s).

The three failure counters are independent — this mirrors Envoy's `splitExternalLocalOriginErrors` so a backend doing graceful 503 overload-shedding is not ejected on the same threshold as a dead port. Limitation: app-origin 502/503/504 are counted as gateway failures (the gateway only sees the status code) — raise `consecutiveGatewayErrors` if your application emits them directly. No-op for UDPRoute.

## How It Works

1. A failure event occurs on a backend endpoint (in one of the three classes) → that class's consecutive failure count +1
2. A success response is received on a backend endpoint → all consecutive failure counts reset to zero
3. Any class's consecutive failure count reaches its threshold → the endpoint is ejected
4. During ejection, load balancing selection skips the endpoint
5. After the backoff-scaled ejection duration elapses → the endpoint automatically recovers and rejoins the selection pool

`maxEjectionPercent` prevents all endpoints from being ejected simultaneously. For example, with a setting of 50%, at most 1 out of 2 endpoints can be ejected.

## Interaction with Retries

Outlier detection counts **upstream attempts**, not client requests. Every completed upstream response is classified before route-level [retry](../resilience/retry.md) decides whether to retry it, so each retried attempt contributes its own success or failure sample.

A rule with `retry.attempts: N` therefore issues up to `N + 1` upstream attempts for one client request, and that single request can contribute up to `N + 1` consecutive failures. Each attempt re-runs endpoint selection, so the samples land on whichever endpoints the load balancer picked; when every attempt hits the same endpoint — a single-endpoint Service, or a request pinned by [strict session persistence](../resilience/session-persistence.md) — all `N + 1` failures accumulate on that one endpoint.

Tuning implications:

- If one client request should not be able to trip ejection on its own, set `consecutiveErrors` / `consecutiveGatewayErrors` above `attempts + 1`.
- `consecutiveLocalOriginFailures` needs the same treatment, and it applies even to rules with no `retry` block: connection failures are retried against `EdgionGatewayConfig.spec.retry.attempts` (default `2`), so by default one client request can contribute up to 3 local-origin failures.
- `maxEjectionPercent` does not protect a single-endpoint Service — the cap is floored at one endpoint, so the only endpoint of a Service can always be ejected, even at the default `maxEjectionPercent: 50`. While it is ejected the gateway has no endpoint left to select and returns a local `503` until the ejection expires.

## Tuning

| Goal | Adjustment | Trade-off |
|------|------------|-----------|
| React faster to a failing endpoint | Lower `consecutiveErrors` (e.g. `3`) | More sensitive to transient blips; an endpoint may be ejected on a brief spike |
| Tolerate occasional errors | Raise `consecutiveErrors` (e.g. `10`) | Slower to remove a genuinely failing endpoint |
| Give a recovering endpoint more time | Raise `ejectionTime` (e.g. `60s`) | Longer reduced capacity while the endpoint is parked |
| Probe recovery quickly | Lower `ejectionTime` (e.g. `10s`) | An endpoint that is still broken re-enters the pool sooner and may be re-ejected |
| Protect capacity on small pools | Lower `maxEjectionPercent` | A failing endpoint may stay in rotation if the cap is already reached |

Recommended starting point for most production Services: `consecutiveErrors: 5`, `ejectionTime: 30s`, `maxEjectionPercent: 50` (the defaults). Tune from there using the metrics below.

⚠️ **On very small pools** (1–2 endpoints), `maxEjectionPercent` offers almost no protection — the cap is floored at one endpoint, so a single-endpoint Service can always be ejected no matter how low the non-zero percentage is. Keep the consecutive thresholds high enough for the rule's retry budget (see [Interaction with Retries](#interaction-with-retries)), and pair outlier detection with active health checks so unhealthy endpoints are still skipped during selection.

## Relationship with Health Checks

| | Health Check | Circuit Breaker (Outlier Detection) |
|--|-------------|-----------------|
| Method | Active probing | Passive statistics |
| Trigger | Periodically sends probes | Actual request failures |
| Recovery | Probe succeeds N times | Automatic recovery after cooldown |
| Configuration | `spec.healthCheck` on `EdgionBackendTrafficPolicy` | `spec.outlierDetection` on `EdgionBackendTrafficPolicy` |

Both live on the same `EdgionBackendTrafficPolicy` resource and can be used simultaneously without conflict.

## Observability

The circuit breaker exposes the following Prometheus metrics:

| Metric | Type | Description |
|--------|------|-------------|
| `edgion_circuit_breaker_ejections_total` | Counter | Total number of ejection events (labelled `class` = `gateway`/`application`/`local_origin`) |
| `edgion_circuit_breaker_ejected_endpoints` | Gauge | Current number of ejected endpoints |
| `edgion_circuit_breaker_rejections_total` | Counter | Number of selections rejected due to ejection |

All metrics carry a `service` label (format: `namespace/name`).

## Related Documentation

- [Backend Active Health Check](./health-check.md)
- [Load Balancing Algorithms](../lb-algorithms.md)
- [Service Reference](./service-ref.md)
