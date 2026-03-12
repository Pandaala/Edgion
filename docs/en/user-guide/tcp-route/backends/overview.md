# TCPRoute Backend Configuration

TCPRoute uses `backendRefs` to point to Layer 4 backend services.

## Example

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: mysql-route
  namespace: gateway-system
spec:
  parentRefs:
    - name: edge-gw
      sectionName: tcp-mysql
  rules:
    - backendRefs:
        - name: mysql
          port: 3306
```

## Notes

1. TCPRoute does not support HTTP-layer filters.
2. For more complex Layer 4 control, use Stream Plugins.
