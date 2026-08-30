---
name: edgion-backend-traffic-policy-features
description: Canonical EdgionBackendTrafficPolicy schema for direct Service and AI EdgionBackend targets.
---

# EdgionBackendTrafficPolicy

EdgionBackendTrafficPolicy configures direct backend resilience. A policy targets either core
Services or AI EdgionBackends in its own Namespace. It does not target HTTPRoute and does not
change the route's backend list, order, or weights.

## Target contract

```yaml
targetRefs:
  - group: edgion.io
    kind: EdgionBackend
    name: primary-provider
```

All targets in one policy must have the same kind. EdgionBackend policies accept 1–16 unique
targets; Service policies retain their existing fan-out behavior.

## Section applicability

| Section | Core Service | AI EdgionBackend |
|---------|--------------|------------------|
| `loadBalancer` | Supported | Rejected |
| `healthCheck` | Supported | Rejected |
| `outlierDetection` | Supported | Supported |
| `retryConstraint` | Supported | Supported |
| `circuitBreaker` | Supported | Supported |
| `upstreamAuthority` | Supported | Rejected |
| `connection` | Supported | Supported |

Unsupported sections reject the whole policy.

For AI traffic, passive outlier state is isolated by typed backend and credential alias.
`circuitBreaker.maxParallelRequests` is an aggregate limit for the direct AI backend.
`connection.connectTimeout` bounds provider connection establishment.
`retryConstraint` is an anti-storm budget; HTTPRoute retry configuration still decides whether a
request is eligible to retry.

## Example

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: provider-resilience
  namespace: production
spec:
  targetRefs:
    - group: edgion.io
      kind: EdgionBackend
      name: primary-provider
  connection:
    connectTimeout: 2s
  retryConstraint:
    budget:
      percent: 20
      interval: 10s
  circuitBreaker:
    maxParallelRequests: 200
  outlierDetection:
    consecutiveErrors: 5
    consecutiveGatewayErrors: 3
    consecutiveLocalOriginFailures: 3
    ejectionTime: 30s
    maxEjectionTime: 5m
```

Oldest creation timestamp wins per typed target; policy name breaks ties. Status reports
`Accepted` and `ResolvedRefs` per Gateway ancestor.
