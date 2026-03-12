# 重试策略

配置请求重试以提高可用性。

## 配置

```yaml
rules:
  - matches:
      - path:
          type: PathPrefix
          value: /api
    retry:
      attempts: 3
      backoff:
        baseInterval: 100ms
        maxInterval: 10s
      retryOn:
        - "5xx"
        - "reset"
        - "connect-failure"
    backendRefs:
      - name: api-service
        port: 8080
```

## 配置参考

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| attempts | int | | 1 | 最大重试次数 |
| backoff.baseInterval | duration | | 25ms | 重试基础间隔 |
| backoff.maxInterval | duration | | 250ms | 重试最大间隔 |
| retryOn | []string | | | 触发重试的条件 |

## 重试条件

| 条件 | 说明 |
|------|------|
| 5xx | HTTP 5xx 响应 |
| reset | 连接重置 |
| connect-failure | 连接失败 |
| retriable-4xx | 可重试的 4xx（408, 429） |
| refused-stream | 流被拒绝 |
| cancelled | 请求被取消 |
| deadline-exceeded | 超时 |
| unavailable | 服务不可用 |

## 示例

### 示例 1: 基本重试

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: retry-example
spec:
  parentRefs:
    - name: my-gateway
  rules:
    - retry:
        attempts: 3
        retryOn:
          - "5xx"
          - "connect-failure"
      backendRefs:
        - name: api-service
          port: 8080
```

### 示例 2: 指数退避

```yaml
retry:
  attempts: 5
  backoff:
    baseInterval: 100ms
    maxInterval: 10s
  retryOn:
    - "5xx"
```

重试间隔：100ms → 200ms → 400ms → 800ms → 1600ms（最大 10s）

## 注意事项

- 只对幂等操作启用重试
- 设置合理的超时配合重试
- 监控重试率，过高可能表示后端问题

## 相关文档

- [超时配置](./timeouts.md)
- [会话保持](./session-persistence.md)
