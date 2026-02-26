# ResponseHeaderModifier

`ResponseHeaderModifier` 是 Gateway API 标准过滤器，用于在响应返回客户端前增删改响应头。

## 示例

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

## 相关文档

- [RequestHeaderModifier](./request-header-modifier.md)
- [过滤器总览](../overview.md)
