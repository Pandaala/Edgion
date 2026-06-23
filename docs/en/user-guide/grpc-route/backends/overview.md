# GRPCRoute Backend Configuration

A GRPCRoute forwards gRPC calls to one or more Kubernetes Services that speak gRPC over HTTP/2. Load balancing across the Service endpoints is automatic; when several backends are listed, traffic is split by weight.

## Basic example

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: user-service-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - backendRefs:
        - name: grpc-service
          port: 9000
```

## Backend fields

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | String | **Required** | Target Kubernetes Service name (must serve gRPC over HTTP/2) |
| `port` | Integer | **Required** | Service port to connect to |
| `weight` | Integer | `1` | Relative share of traffic sent to this backend |
| `namespace` | String | GRPCRoute namespace | Service namespace; cross-namespace requires a `ReferenceGrant` |
| `kind` | String | `Service` | Backend object kind |

## Weighted backends

List multiple `backendRefs` to split calls across backends by weight (a ratio, not a percentage) — useful for canary rollouts of a new service version:

```yaml
spec:
  rules:
    - backendRefs:
        - name: user-service-stable
          port: 9000
          weight: 9
        - name: user-service-canary
          port: 9000
          weight: 1
```

A backend with `weight: 0` receives no new calls.

## Cross-namespace backends

To forward to a Service in another namespace, set `namespace` on the `backendRef` and create a `ReferenceGrant` in the target namespace permitting `GRPCRoute` to reference `Service`.

## Related documents

- [GRPCRoute Overview](../overview.md)
- [GRPCRoute Matching Rules](../matches/overview.md)
- [Cross-Namespace Reference](../../../ops-guide/infrastructure/reference-grant.md)
