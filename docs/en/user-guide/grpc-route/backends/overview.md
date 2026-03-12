# GRPCRoute Backend Configuration

GRPCRoute specifies gRPC backend services through `backendRefs`.

## Minimal Example

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: order-route
  namespace: gateway-system
spec:
  parentRefs:
    - name: edge-gw
  rules:
    - backendRefs:
        - name: order-service
          port: 50051
          weight: 100
```

## Recommendations

1. For canary release scenarios, use `weight` for traffic splitting.
2. If backend TLS is needed, manage certificates uniformly following the approach described in the HTTPRoute backend TLS documentation.

## Related Documentation

- [Service Reference (HTTPRoute)](../../http-route/backends/service-ref.md)
- [Weight Configuration (HTTPRoute)](../../http-route/backends/weight.md)
