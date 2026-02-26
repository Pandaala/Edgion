# RequestHeaderModifier

`RequestHeaderModifier` 是 Gateway API 标准过滤器，用于在请求转发前增删改请求头。

## 示例

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

## 相关文档

- [ResponseHeaderModifier](./response-header-modifier.md)
- [过滤器总览](../overview.md)
