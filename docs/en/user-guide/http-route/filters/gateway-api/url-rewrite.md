# URLRewrite

`URLRewrite` is a Gateway API standard filter used to rewrite the hostname or path of a request.

## Example: Rewrite Prefix Path

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: rewrite-demo
  namespace: app
spec:
  parentRefs:
    - name: edge-gw
      namespace: gateway-system
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api
      filters:
        - type: URLRewrite
          urlRewrite:
            path:
              type: ReplacePrefixMatch
              replacePrefixMatch: /v1
      backendRefs:
        - name: app-svc
          port: 8080
```

## Related Documentation

- [RequestRedirect](./request-redirect.md)
- [ProxyRewrite (Edgion Extension)](../edgion-plugins/proxy-rewrite.md)
