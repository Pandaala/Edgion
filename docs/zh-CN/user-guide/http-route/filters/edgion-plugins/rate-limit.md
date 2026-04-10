# RateLimit（HTTPRoute 过滤器）

> **🔌 Edgion 扩展**
> 
> RateLimit 是 `EdgionPlugins` CRD 提供的速率限制插件，不属于标准 Gateway API。

RateLimit 可以作为 HTTPRoute 的过滤器使用，也可以通过 `EdgionPlugins` 全局配置。本页介绍如何在 HTTPRoute 中使用；完整配置参数请见 [独立插件页](../../edgion-plugins/rate-limit.md)。

## 在 HTTPRoute 中使用

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: api-rate-limit
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: api-rate-limit
  namespace: production
spec:
  plugins:
    - RateLimit:
        key:
          - type: clientIp
        limit: 100
        window: "60s"
```

## 相关链接

- [RateLimit 独立插件页（完整配置参数）](../../edgion-plugins/rate-limit.md)
- [RateLimitRedis（Redis 精确集群限流）](./rate-limit-redis.md)
- [IP 限制](./ip-restriction.md)
- [插件组合与引用](../plugin-composition.md)
