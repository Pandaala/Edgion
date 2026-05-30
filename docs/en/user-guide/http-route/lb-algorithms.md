# Load Balancing Algorithms

> **🔌 Edgion extension**
> 
> Selecting a non-default load balancing algorithm is an Edgion extension expressed through `LBPolicy*` ExtensionRefs on `backendRefs[].filters`; it is not part of standard Gateway API.

Edgion distributes requests across a backend's endpoints. By default it uses weighted round-robin; you can opt a backend into consistent hashing, least-connection, or latency-aware EWMA selection.

## Supported algorithms

| Algorithm | How it selects | When to use |
|-----------|----------------|-------------|
| RoundRobin (default) | Weighted round-robin across healthy endpoints | General-purpose HTTP traffic |
| ConsistentHash | Hashes a request attribute onto a Ketama ring | Cache affinity, sticky-by-key routing |
| LeastConnection | Endpoint with the fewest in-flight connections | Long-lived connections (gRPC streams, WebSocket) |
| EWMA | Endpoint with the lowest exponentially weighted moving average latency | Heterogeneous backends with uneven latency |

When no policy is configured, the default weighted round-robin selector is used.

## How to configure

A load balancing policy is declared as an `ExtensionRef` filter **on a backendRef** (not on the rule). The filter uses `group: edgion.io` and one of the `LBPolicy*` kinds. These ExtensionRefs are recognized by their `kind` and consumed before plugin processing, so they never reach the plugin pipeline.

| `kind` | Meaning | `name` field |
|--------|---------|--------------|
| `LBPolicyConsistentHash` | Consistent hashing | Selects the hash key (see below) |
| `LBPolicyLeastConn` | Least connections | `default` (value is ignored) |
| `LBPolicyEwma` | EWMA latency selection | `default` (value is ignored) |

`group` may also be left empty (the core API group) — it is accepted as equivalent to `edgion.io`. Any other group, or an unrecognized `kind`, is ignored and the backend keeps the default RoundRobin algorithm.

### Minimal example: least connections

```yaml
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
          filters:
            - type: ExtensionRef
              extensionRef:
                group: edgion.io
                kind: LBPolicyLeastConn
                name: default
```

## Algorithm details

### RoundRobin (default)

- Weighted round-robin: a higher `weight` on a backendRef receives proportionally more traffic.
- Lock-free O(1) selection with backend health filtering and fallback to healthy endpoints.
- Weight is interpreted as a **ratio** — only the relative proportion between backends matters, not the absolute magnitude. `[2, 4]` behaves identically to `[1, 2]`. Very large weights are reduced (by GCD) and, if still large, scaled down proportionally, so the effective ratio resolution is about 1/10,000 (0.01%). This only affects splits finer than that, far below any practical canary configuration.

### ConsistentHash

- Ketama-based consistent hash ring: the same hash key maps to the same endpoint as long as the endpoint set is unchanged.
- When endpoints change, only roughly 1/N of keys remap.
- The hash key comes from a request attribute selected by the `name` field.
- If the selected attribute is **missing on a request** (for example, the named header is not present), the hash key is empty and that request falls back to round-robin selection.

The `name` field selects the hash key using a `<source>.<key>` form:

| `name` value | Hash key |
|--------------|----------|
| `header.<HeaderName>` | The value of that request header |
| `cookie.<CookieName>` | The value of that cookie |
| `arg.<QueryParam>` | The value of that query-string argument |

The `<source>` prefix must be exactly `header`, `cookie`, or `arg`. A `name` that does not parse into one of these forms causes the policy to be **ignored**, and the backend keeps the default RoundRobin algorithm.

```yaml
backendRefs:
  - name: cache-service
    port: 6379
    filters:
      - type: ExtensionRef
        extensionRef:
          group: edgion.io
          kind: LBPolicyConsistentHash
          # Hash on the X-User-Id header
          name: header.X-User-Id
```

### LeastConnection

- Selects the endpoint with the fewest active connections.
- Connection counting is service-scoped: the same endpoint IP under different services is tracked independently.
- The counter increments when a request connects and decrements when it completes; retried attempts decrement correctly so a retry does not leak a count.
- New endpoints (count 0) are preferred; removed endpoints drain gracefully — no new requests, existing ones finish.

### EWMA

- Selects the endpoint with the lowest exponentially weighted moving average of latency.
- Update formula: `new = alpha × latency + (1 - alpha) × old`, with a default alpha of 10%.
- Latency is folded in when each request completes; retried attempts drop their sample so they do not pollute the previous endpoint's average.
- New endpoints start at 1ms (briefly preferred) and converge to their real latency.
- State is service-scoped.

## Configuration scenarios

### Cache service with consistent hashing by cookie

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: cache-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - "cache.example.com"
  rules:
    - backendRefs:
        - name: cache-service
          port: 6379
          filters:
            - type: ExtensionRef
              extensionRef:
                group: edgion.io
                kind: LBPolicyConsistentHash
                name: cookie.session_id
```

### gRPC service with EWMA

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: grpc-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /grpc
      backendRefs:
        - name: grpc-service
          port: 50051
          filters:
            - type: ExtensionRef
              extensionRef:
                group: edgion.io
                kind: LBPolicyEwma
                name: default
```

### Weighted canary with the default algorithm

No policy filter is needed — weighted round-robin is the default:

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

- **Scope**: a policy applies to the service named by the backendRef it is attached to.
- **Shared services**: when several routes attach different policies to the same service, the service honors the union of those policies; the LB layer picks the appropriate one per request. A policy is removed only when the last route referencing it is deleted.
- **Health checks**: all algorithms filter out unhealthy endpoints automatically.
- **Weight**: all algorithms honor the `weight` field on backendRefs.

## Related documents

- [Backend configuration](./backends/README.md)
- [Session persistence](./resilience/session-persistence.md)
- [HTTPRoute overview](./overview.md)
