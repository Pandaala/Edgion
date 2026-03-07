# URLRewrite

`URLRewrite` 是 Gateway API 标准过滤器，用于重写请求的主机名或路径。

## 示例：重写前缀路径

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

## 相关文档

- [RequestRedirect](./request-redirect.md)
- [ProxyRewrite（Edgion 扩展）](../edgion-plugins/proxy-rewrite.md)
