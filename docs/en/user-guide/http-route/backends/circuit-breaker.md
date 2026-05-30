# Circuit Breaker

> **🔌 Edgion extension**
>
> This feature enables passive ejection for backends via the `edgion.io/circuit-breaker` Annotation, and is an Edgion extension capability.

## Overview

The circuit breaker tracks consecutive failures for each backend endpoint on the gateway side. When the threshold is reached, the endpoint is automatically ejected; after the cooldown period expires, it automatically recovers.
This feature is used to prevent cascading failures and protect upstream services, complementing active health checks.

**Failure events** include: connection failures, proxy errors, and upstream 5xx status codes.
**Success events** (non-5xx responses) reset the consecutive failure count.

## Quick Start

Add the annotation to a `Service` to enable it:

```yaml
apiVersion: v1
kind: Service
metadata:
  name: my-backend
  namespace: default
  annotations:
    edgion.io/circuit-breaker: |
      consecutiveErrors: 5
      ejectionSeconds: 30
      maxEjectionPercent: 50
spec:
  ports:
    - port: 8080
      targetPort: 8080
```

## Configuration Reference

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `consecutiveErrors` | Integer | `5` | Consecutive failures before an endpoint is ejected. Minimum `1` |
| `ejectionSeconds` | Integer | `30` | Cooldown in seconds before an ejected endpoint rejoins. Minimum `1` |
| `maxEjectionPercent` | Integer | `50` | Maximum percent of endpoints that may be ejected at once. `0` means no limit; values above `100` are rejected |

An annotation that fails to parse or fails validation (for example `consecutiveErrors: 0`, `ejectionSeconds: 0`, or `maxEjectionPercent: 150`) is logged and **ignored**, leaving the circuit breaker disabled for that Service.

## How It Works

1. A failure event occurs on a backend endpoint -> consecutive failure count +1
2. A success response is received on a backend endpoint -> consecutive failure count resets to zero
3. Consecutive failure count reaches `consecutiveErrors` -> the endpoint is ejected
4. During ejection, load balancing selection skips the endpoint
5. After `ejectionSeconds` seconds elapse -> the endpoint automatically recovers and rejoins the selection pool

`maxEjectionPercent` prevents all endpoints from being ejected simultaneously. For example, with a setting of 50%, at most 1 out of 2 endpoints can be ejected.

## Tuning

| Goal | Adjustment | Trade-off |
|------|------------|-----------|
| React faster to a failing endpoint | Lower `consecutiveErrors` (e.g. `3`) | More sensitive to transient blips; an endpoint may be ejected on a brief spike |
| Tolerate occasional errors | Raise `consecutiveErrors` (e.g. `10`) | Slower to remove a genuinely failing endpoint |
| Give a recovering endpoint more time | Raise `ejectionSeconds` (e.g. `60`) | Longer reduced capacity while the endpoint is parked |
| Probe recovery quickly | Lower `ejectionSeconds` (e.g. `10`) | An endpoint that is still broken re-enters the pool sooner and may be re-ejected |
| Protect capacity on small pools | Lower `maxEjectionPercent` | A failing endpoint may stay in rotation if the cap is already reached |

Recommended starting point for most production Services: `consecutiveErrors: 5`, `ejectionSeconds: 30`, `maxEjectionPercent: 50` (the defaults). Tune from there using the metrics below.

⚠️ **On very small pools** (1–2 endpoints), a low `maxEjectionPercent` can prevent ejection entirely — the cap is reached before a single endpoint is removed. Pair the circuit breaker with active health checks so unhealthy endpoints are still skipped during selection.

## Relationship with Health Checks

| | Health Check | Circuit Breaker |
|--|-------------|-----------------|
| Method | Active probing | Passive statistics |
| Trigger | Periodically sends probes | Actual request failures |
| Recovery | Probe succeeds N times | Automatic recovery after cooldown |
| Configuration | `edgion.io/health-check` | `edgion.io/circuit-breaker` |

Both can be used simultaneously and complement each other without conflict.

## Observability

The circuit breaker exposes the following Prometheus metrics:

| Metric | Type | Description |
|--------|------|-------------|
| `edgion_circuit_breaker_ejections_total` | Counter | Total number of ejection events |
| `edgion_circuit_breaker_ejected_endpoints` | Gauge | Current number of ejected endpoints |
| `edgion_circuit_breaker_rejections_total` | Counter | Number of selections rejected due to ejection |

All metrics carry a `service` label (format: `namespace/name`).
