# Service Reference

A `backendRef` points an HTTPRoute rule at a Kubernetes Service. The Gateway resolves that Service to its live endpoints and load-balances requests across them.

## Basic Configuration

```yaml
backendRefs:
  - name: api-service
    port: 8080
```

## Configuration Reference

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | String | **Required** | Service name |
| `port` | Integer | **Required** | Service port (the `port` exposed by the Service, not the Pod `targetPort`) |
| `namespace` | String | Route's namespace | Service namespace; cross-namespace needs a ReferenceGrant |
| `kind` | String | `Service` | Backend kind. Allowed values: `Service` \| `ServiceClusterIp` \| `ServiceExternalName` |
| `group` | String | `""` | API group; `""` for core Services |
| `weight` | Integer | `1` | Relative traffic weight. See [Weight Configuration](./weight.md) |

## How a backendRef resolves to endpoints

1. The `name` (+ `namespace`) identifies the Service.
2. The `port` selects the matching Service port. Edgion maps it to the Pod-facing `targetPort` and the backing EndpointSlices.
3. The Controller watches the Service's EndpointSlices and pushes the ready endpoint addresses to the Gateway over gRPC.
4. The Gateway load-balances requests across those endpoints, skipping any that health checks or the circuit breaker have marked unhealthy.

Endpoint membership comes from the EndpointSlice API (not the legacy single `Endpoints` object), so endpoint sets scale and update incrementally. When Pods are added, removed, or change readiness, the updated EndpointSlice flows through to the Gateway without re-applying the route.

A Service change (such as a port rename) automatically triggers reprocessing of every route that references it, so dependent routes stay consistent.

## Default Behavior

### `kind` field

- When `kind` is not specified, it defaults to `Service`.
- `ServiceExternalName` resolves the Service's `externalName` (DNS) target rather than in-cluster EndpointSlices.

### `namespace` field

- When `namespace` is omitted, the Gateway uses the HTTPRoute's own namespace.
- Referencing a Service in a different namespace requires a ReferenceGrant in the **target** namespace.

### Unresolved backends

If a referenced Service or port cannot be resolved (wrong name, missing ReferenceGrant, or no ready endpoints), the route's `ResolvedRefs` condition reflects the failure and requests to that backend return a 5xx. Fix the reference and the Gateway recovers automatically once endpoints become available.

## Cross-Namespace References

Referencing a Service in another namespace requires a ReferenceGrant created in the **Service's** namespace. Without it, the reference is rejected even if the Service exists:

```yaml
# HTTPRoute in the `production` namespace
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - backendRefs:
        - name: backend-service
          namespace: staging
          port: 8080

---
# ReferenceGrant created in the target namespace (`staging`)
apiVersion: gateway.networking.k8s.io/v1
kind: ReferenceGrant
metadata:
  name: allow-from-production
  namespace: staging
spec:
  from:
    - group: gateway.networking.k8s.io
      kind: HTTPRoute
      namespace: production
  to:
    - group: ""
      kind: Service
```

## Related Documentation

- [Weight Configuration](./weight.md)
- [Backend TLS](./backend-tls.md)
- [Backend Active Health Check](./health-check.md)
- [Circuit Breaker](./circuit-breaker.md)
