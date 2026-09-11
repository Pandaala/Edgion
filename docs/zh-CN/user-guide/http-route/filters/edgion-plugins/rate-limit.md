# RateLimit（HTTPRoute 过滤器）

> **🔌 Edgion 扩展**
> 
> RateLimit 是 `EdgionPlugins` CRD 提供的速率限制插件，不属于标准 Gateway API。

RateLimit 可作为 HTTPRoute 过滤器使用，也可通过 `EdgionPlugins` 全局配置。本页介绍如何在 HTTPRoute 中使用；完整配置参数请见[独立插件页](../../../edgion-plugins/rate-limit.md)。

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
  requestPlugins:
    - type: RateLimit
      config:
        # 每个时间窗口允许的最大请求数
        rate: 100
        interval: "60s"
        key:
          - type: remoteIp
```

> **Schema 说明**：RateLimit 在 `requestPlugins` 阶段运行，使用 `type` / `config` 插件条目格式。字段为 `rate`（每个时间窗口的最大请求数）和 `interval`（时间窗口）——没有 `limit` / `window` 字段。完整参数表请见[独立插件页](../../../edgion-plugins/rate-limit.md)。

## 相关链接

- [RateLimit 独立插件页（完整配置参数）](../../../edgion-plugins/rate-limit.md)
- [RateLimitRedis（Redis 精确集群限流）](./rate-limit-redis.md)
- [IP 限制](./ip-restriction.md)
- [插件组合与引用](../plugin-composition.md)
## Redis 补充限流

通过 `onLimitExceeded: Continue` 和后续 Redis 条目的 `rateLimitExceeded` 条件，
让本地通过的请求不访问 Redis，仅超额请求使用共享补充额度。
参见[完整配置](./rate-limit-redis.md#本地额度与-redis-补充准入)。
