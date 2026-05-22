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

## Behavior

The original request query string is preserved and appended to the redirect `Location`
URL. For example, a request to `http://app.example.com/login?returnTo=/dashboard` redirects
to `https://app.example.com/login?returnTo=/dashboard`. This applies regardless of any path
modifier (`replaceFullPath` / `replacePrefixMatch`); the Gateway API redirect filter has no
query modifier, so the client's query always survives the redirect.

## Related Documentation

- [URLRewrite](./url-rewrite.md)
- [HTTP to HTTPS Redirect (Gateway Level)](../../../../ops-guide/gateway/http-to-https-redirect.md)
