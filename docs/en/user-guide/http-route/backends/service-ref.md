# Service Reference

Configure backend service references.

## Basic Configuration

```yaml
backendRefs:
  - name: my-service
    port: 8080
```

## Configuration Reference

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| name | string | ✓ | | Service name |
| namespace | string | | Same as route | Service namespace |
| port | int | ✓ | | Service port |
| kind | string | | Service | Backend type |
| weight | int | | 1 | Weight (for traffic distribution) |

## Default Behavior

### kind Field

- When `kind` is not specified, it defaults to `Service`
- Supported types: `Service`, `ServiceClusterIp`, `ServiceExternalName`

### namespace Field

- When `namespace` is not specified, the namespace of the HTTPRoute is used
- Cross-namespace references require a ReferenceGrant

## Cross-Namespace References

Referencing a Service in another namespace requires a ReferenceGrant:

```yaml
# Route configuration
backendRefs:
  - name: backend-service
    namespace: backend-ns
    port: 8080

---
# ReferenceGrant (created in backend-ns)
apiVersion: gateway.networking.k8s.io/v1beta1
kind: ReferenceGrant
metadata:
  name: allow-from-default
  namespace: backend-ns
spec:
  from:
    - group: gateway.networking.k8s.io
      kind: HTTPRoute
      namespace: default
  to:
    - group: ""
      kind: Service
```

## Related Documentation

- [Weight Configuration](./weight.md)
- [Backend TLS](./backend-tls.md)
