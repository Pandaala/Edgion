# Load Balancing Algorithms

> **🔌 Edgion extension**
>
> Selecting a non-default load balancing algorithm is an Edgion extension expressed through the `spec.loadBalancer` section of an `EdgionBackendTrafficPolicy` custom resource attached to a Service; it is not part of standard Gateway API.

Edgion distributes requests across a backend's endpoints. By default it uses weighted round-robin; you can opt a **Service** into consistent hashing, least-connection, or latency-aware EWMA selection.

`EdgionBackendTrafficPolicy` also carries [active health checks](./backends/health-check.md) and [outlier detection](./backends/circuit-breaker.md) for the same Service; see the [full schema reference](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/20-edgion-backend-traffic-policy.md) for all three sections together.

## Supported algorithms

| Algorithm | How it selects | When to use |
|-----------|----------------|-------------|
| RoundRobin (default) | Weighted round-robin across healthy endpoints | General-purpose HTTP traffic |
| ConsistentHash | Hashes a request attribute onto a Ketama ring | Cache affinity, sticky-by-key routing |
| LeastConn | Endpoint with the fewest in-flight connections | Long-lived connections (gRPC streams, WebSocket) |
| Ewma | Endpoint with the lowest exponentially weighted moving average latency | Heterogeneous backends with uneven latency |

When no policy is configured, or `loadBalancer.type: RoundRobin`, the default weighted round-robin selector is used.

## How to configure

A load balancing policy is declared in `spec.loadBalancer` on an `EdgionBackendTrafficPolicy` resource, attached to a Service via `spec.targetRefs` (same namespace as the policy). This is a **per-destination** setting: one algorithm applies to a Service regardless of which route(s) send traffic to it.

| `loadBalancer.type` | Meaning | `consistentHash` |
|--------|---------|--------------|
| `RoundRobin` | Weighted round-robin (default) | Not applicable |
| `LeastConn` | Least connections | Not applicable |
| `Ewma` | EWMA latency selection | Not applicable |
| `ConsistentHash` | Consistent hashing | Required — selects the hash key (see below) |

`consistentHash` is required when `type: ConsistentHash` and rejected (spec invalid) for any other type.

### Minimal example: least connections

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: api-lb-policy
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: api-service
  loadBalancer:
    type: LeastConn
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - "api.example.com"
  rules:
    - backendRefs:
        - name: api-service
          port: 8080
```

## Algorithm details

### RoundRobin (default)

- Weighted round-robin: a higher `weight` on a backendRef receives proportionally more traffic.
- Lock-free O(1) selection with backend health filtering and fallback to healthy endpoints.
- Weight is interpreted as a **ratio** — only the relative proportion between backends matters, not the absolute magnitude. `[2, 4]` behaves identically to `[1, 2]`. Very large weights are reduced (by GCD) and, if still large, scaled down proportionally, so the effective ratio resolution is about 1/10,000 (0.01%). This only affects splits finer than that, far below any practical canary configuration.

### ConsistentHash

- Ketama-based consistent hash ring: the same hash key maps to the same endpoint as long as the endpoint set is unchanged.
- When endpoints change, only roughly 1/N of keys remap.
- The hash key comes from a request attribute selected by `consistentHash.hashOn` + `consistentHash.key`.
- If the selected attribute is **missing on a request** (for example, the named header is not present), the hash key is empty and that request falls back to round-robin selection.

`consistentHash.hashOn` selects the hash source, and `consistentHash.key` names the specific header / cookie / query parameter:

| `hashOn` value | Hash key |
|--------------|----------|
| `header` | The value of the request header named by `key` |
| `cookie` | The value of the cookie named by `key` |
| `queryParam` | The value of the query-string argument named by `key` |

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: cache-lb-policy
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: cache-service
  loadBalancer:
    type: ConsistentHash
    consistentHash:
      hashOn: header
      key: X-User-Id
```

### LeastConn

- Selects the endpoint with the fewest active connections.
- Connection counting is service-scoped: the same endpoint IP under different services is tracked independently.
- The counter increments when a request connects and decrements when it completes; retried attempts decrement correctly so a retry does not leak a count.
- New endpoints (count 0) are preferred; removed endpoints drain gracefully — no new requests, existing ones finish.

### Ewma

- Selects the endpoint with the lowest exponentially weighted moving average of latency.
- Update formula: `new = alpha × latency + (1 - alpha) × old`, with a default alpha of 10%.
- Latency is folded in when each request completes; retried attempts drop their sample so they do not pollute the previous endpoint's average.
- New endpoints start at 1ms (briefly preferred) and converge to their real latency.
- State is service-scoped.

## Configuration scenarios

### Cache service with consistent hashing by cookie

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: cache-lb-policy
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: cache-service
  loadBalancer:
    type: ConsistentHash
    consistentHash:
      hashOn: cookie
      key: session_id
```

### gRPC service with EWMA

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: grpc-lb-policy
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: grpc-service
  loadBalancer:
    type: Ewma
```

### Weighted canary with the default algorithm

No `EdgionBackendTrafficPolicy` is needed — weighted round-robin is the default. Weight is still expressed on the route's `backendRefs`:

```yaml
rules:
  - backendRefs:
      - name: stable-service
        port: 8080
        weight: 90
      - name: canary-service
        port: 8080
        weight: 10
```

## Notes

- **Scope**: a policy applies to every Service named in its `targetRefs` (same namespace as the policy) — this is per-**destination**, not per-route. Every route sending traffic to that Service uses the same algorithm.
- **Conflicts**: when more than one `EdgionBackendTrafficPolicy` targets the same Service, the **oldest** `creationTimestamp` wins (ties break by name); the loser's status reports `Conflicted=True`. There is no per-field merge or union across policies — see [Conflict Resolution](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/20-edgion-backend-traffic-policy.md#conflict-resolution).
- **Health checks and outlier detection**: all algorithms filter out unhealthy / ejected endpoints automatically (configured in the same policy's `healthCheck` / `outlierDetection` sections).
- **Weight**: all algorithms honor the `weight` field on backendRefs.

## Related documents

- [Backend configuration](./backends/README.md)
- [Backend Active Health Check](./backends/health-check.md)
- [Circuit Breaker (Outlier Detection)](./backends/circuit-breaker.md)
- [Session persistence](./resilience/session-persistence.md)
- [HTTPRoute overview](./overview.md)
