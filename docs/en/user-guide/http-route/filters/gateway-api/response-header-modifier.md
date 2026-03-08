# ResponseHeaderModifier

`ResponseHeaderModifier` is a Gateway API standard filter used to add, set, or remove response headers before returning the response to the client.

## Example

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: response-header-demo
  namespace: app
spec:
  parentRefs:
    - name: edge-gw
      namespace: gateway-system
  rules:
    - filters:
        - type: ResponseHeaderModifier
          responseHeaderModifier:
            add:
              - name: X-Gateway
                value: edgion
            remove:
              - Server
      backendRefs:
        - name: app-svc
          port: 8080
```

## Related Documentation

- [RequestHeaderModifier](./request-header-modifier.md)
- [Filters Overview](../overview.md)
