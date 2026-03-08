# RequestHeaderModifier

`RequestHeaderModifier` is a Gateway API standard filter used to add, set, or remove request headers before forwarding the request.

## Example

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: header-demo
  namespace: app
spec:
  parentRefs:
    - name: edge-gw
      namespace: gateway-system
  rules:
    - filters:
        - type: RequestHeaderModifier
          requestHeaderModifier:
            add:
              - name: X-Env
                value: prod
            set:
              - name: X-Trace-Source
                value: gateway
            remove:
              - X-Debug
      backendRefs:
        - name: app-svc
          port: 8080
```

## Related Documentation

- [ResponseHeaderModifier](./response-header-modifier.md)
- [Filters Overview](../overview.md)
