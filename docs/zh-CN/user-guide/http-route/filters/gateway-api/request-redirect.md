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

## 行为说明

原始请求的 query string 会被保留并追加到重定向 `Location` URL 上。例如，请求
`http://app.example.com/login?returnTo=/dashboard` 会重定向到
`https://app.example.com/login?returnTo=/dashboard`。无论是否配置了路径修改器
（`replaceFullPath` / `replacePrefixMatch`）都适用；Gateway API 重定向过滤器没有
query 修改器，因此客户端的 query 始终会在重定向中保留。

## 相关文档

- [URLRewrite](./url-rewrite.md)
- [HTTP to HTTPS 重定向（Gateway 级）](../../../../ops-guide/gateway/http-to-https-redirect.md)
