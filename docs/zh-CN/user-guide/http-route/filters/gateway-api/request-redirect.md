# RequestRedirect

`RequestRedirect` 是 Gateway API 标准过滤器，用于将请求重定向到新的 URL。

## 示例：HTTP 跳转 HTTPS

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

## 相关文档

- [URLRewrite](./url-rewrite.md)
- [HTTP to HTTPS 重定向（Gateway 级）](../../../../ops-guide/gateway/http-to-https-redirect.md)
