# RequestRedirect

`RequestRedirect` is a Gateway API standard filter used to redirect requests to a new URL.

## Example: HTTP to HTTPS Redirect

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: redirect-demo
  namespace: app
spec:
  parentRefs:
    - name: edge-gw
      namespace: gateway-system
  rules:
    - filters:
        - type: RequestRedirect
          requestRedirect:
            scheme: https
            statusCode: 301
      backendRefs:
        - name: app-svc
          port: 8080
```

## Related Documentation

- [URLRewrite](./url-rewrite.md)
- [HTTP to HTTPS Redirect (Gateway Level)](../../../../ops-guide/gateway/http-to-https-redirect.md)
