# 重试策略

重试会将失败的请求重新发送到后端，以应对瞬时故障并提高可用性。Edgion 实现了 Gateway API v1.4 的 `rules[].retry` 字段。

## 配置

```yaml
rules:
  - matches:
      - path:
          type: PathPrefix
          value: /api
    retry:
      # 最大重试次数（0 表示禁用重试）
      attempts: 3
      # 重试之间的最小等待时间；每次尝试翻倍，上限 30s
      backoff: "1s"
      # 触发重试的 HTTP 状态码
      codes:
        - 502
        - 503
        - 504
    backendRefs:
      - name: api-service
        port: 8080
```

## 配置参考

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `attempts` | Integer | `EdgionGatewayConfig.spec.maxRetries`（默认 `3`） | 最大重试次数。`0` 禁用重试。省略时使用全局默认值 |
| `backoff` | Duration | 无 | 重试之间的最小等待时间；作为指数退避的基数。示例：`"1s"`、`"100ms"`。省略时重试不等待 |
| `codes` | Array\<Integer\> | 无 | 触发重试的 HTTP 状态码（100–599）。超出范围的状态码被忽略。省略时仅重试连接错误 |

## 重试行为

### 触发条件

- **连接错误**：只要重试策略生效，连接错误就会触发重试（连接失败、在收到任何响应前被重置）。
- **状态码**：只有列在 `codes` 中的状态码才会触发重试。省略 `codes` 时，状态码重试关闭，只重试连接错误。
- 连接失败重试和状态码重试共享同一个 `attempts` 计数器。

### 重试次数预算

有效预算为 `rule.retry.attempts`（若已设置），否则使用全局 `EdgionGatewayConfig.spec.maxRetries`（默认 `3`）。设置 `attempts: 0` 可为该规则禁用重试。

### 退避

设置 `backoff` 后，第 `n` 次重试前的等待时间（从第一次重试开始计数）为 `backoff × 2^(n-1)`，上限 30 秒：

```yaml
retry:
  attempts: 5
  backoff: "100ms"
  codes:
    - 503
```

等待时序：100ms → 200ms → 400ms → 800ms → 1600ms（每次上限 30s）。省略 `backoff` 时，重试之间不等待，立即发起。

### 与超时的交互

`timeouts.request` 的值约束**整个**请求的时间，包括所有重试和退避等待。如果累计时间超过 `request`，则无论剩余重试次数如何，请求都会被终止。请将 `request` 设置得足够大，以允许配置的重试次数完成。详见 [超时配置](./timeouts.md)。

## 示例

### 示例 1：仅对连接失败重试

省略 `codes` 则仅重试连接错误（对任何后端都安全）：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: retry-conn
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - retry:
        attempts: 2
      backendRefs:
        - name: api-service
          port: 8080
```

### 示例 2：对 5xx 启用指数退避重试

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: retry-5xx
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - timeouts:
        request: "30s"
      retry:
        attempts: 3
        backoff: "200ms"
        codes:
          - 500
          - 502
          - 503
          - 504
      backendRefs:
        - name: api-service
          port: 8080
```

### 示例 3：为某条规则禁用重试

```yaml
rules:
  - retry:
      attempts: 0
    backendRefs:
      - name: non-idempotent-service
        port: 8080
```

## 注意事项

- 仅对幂等操作启用状态码重试；对非幂等的 `POST` 重试可能产生重复的副作用。
- 始终配合 `timeouts.request` 使用重试，以限制累计重试时间。
- 持续过高的重试率通常说明后端存在问题，而非瞬时错误。

## 相关文档

- [超时配置](./timeouts.md)
- [会话保持](./session-persistence.md)
- [负载均衡算法](../lb-algorithms.md)
