# TCPRoute Backend Configuration

A TCPRoute forwards a raw TCP stream to one or more Kubernetes Services. Load balancing across the Service endpoints is automatic; when several backends are listed, traffic is split by weight.

## Basic example

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: database-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: postgres
  rules:
    - backendRefs:
        - name: database-service
          port: 5432
```

## Backend fields

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | String | **Required** | Target Kubernetes Service name |
| `port` | Integer | **Required** | Service port to connect to |
| `weight` | Integer | `1` | Relative share of connections sent to this backend |
| `namespace` | String | TCPRoute namespace | Service namespace; cross-namespace requires a `ReferenceGrant` |
| `kind` | String | `Service` | Backend object kind |

## Weighted backends

List multiple `backendRefs` to split new connections across backends. Weight is a ratio, not a percentage — `weight: 3` and `weight: 1` send roughly 75% / 25% of connections:

```yaml
spec:
  rules:
    - backendRefs:
        - name: database-primary
          port: 5432
          weight: 3
        - name: database-canary
          port: 5432
          weight: 1
```

A backend with `weight: 0` receives no new connections, which is useful for draining a backend before removing it.

## Cross-namespace backends

To forward to a Service in another namespace, set `namespace` on the `backendRef` and create a `ReferenceGrant` in the target namespace that permits `TCPRoute` to reference `Service`:

```yaml
apiVersion: gateway.networking.k8s.io/v1beta1
kind: ReferenceGrant
metadata:
  name: allow-tcp-to-db
  namespace: data
spec:
  from:
    - group: gateway.networking.k8s.io
      kind: TCPRoute
      namespace: production
  to:
    - group: ""
      kind: Service
```

## Related documents

- [TCPRoute Overview](../overview.md)
- [Stream Plugins](../stream-plugins.md)
- [Cross-Namespace Reference](../../../ops-guide/infrastructure/reference-grant.md)
