# Circuit Breaker

> **🔌 Edgion Extension**
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
| `consecutiveErrors` | u32 | 5 | Number of consecutive failures before ejection (minimum 1) |
| `ejectionSeconds` | u64 | 30 | Ejection cooldown duration in seconds (minimum 1) |
| `maxEjectionPercent` | u32 | 50 | Maximum ejection percentage (0 = no limit) |

## How It Works

1. A failure event occurs on a backend endpoint -> consecutive failure count +1
2. A success response is received on a backend endpoint -> consecutive failure count resets to zero
3. Consecutive failure count reaches `consecutiveErrors` -> the endpoint is ejected
4. During ejection, load balancing selection skips the endpoint
5. After `ejectionSeconds` seconds elapse -> the endpoint automatically recovers and rejoins the selection pool

`maxEjectionPercent` prevents all endpoints from being ejected simultaneously. For example, with a setting of 50%, at most 1 out of 2 endpoints can be ejected.

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
