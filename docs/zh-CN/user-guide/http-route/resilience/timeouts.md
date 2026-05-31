# 超时配置

超时配置限定网关等待请求完成的时间，防止后端卡住或上游响应缓慢。Edgion 实现了 Gateway API v1.4 的 `rules[].timeouts` 字段。

## 配置

```yaml
rules:
  - matches:
      - path:
          type: PathPrefix
          value: /api
    timeouts:
      # 端到端超时，包含所有重试
      request: "30s"
      # 单次后端请求超时
      backendRequest: "10s"
    backendRefs:
      - name: api-service
        port: 8080
```

## 配置参考

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `request` | Duration | EdgionGatewayConfig 全局值 | 从收到请求到返回响应的总时间，**包含所有重试** |
| `backendRequest` | Duration | EdgionGatewayConfig 全局值 | 单次后端请求的超时时间（一次尝试，不含重试） |

两个字段均接受 Gateway API 时长字符串，如 `"500ms"`、`"10s"`、`"1m"`、`"1h"`。

## 超时类型说明

### request

整个事务的端到端时间预算，涵盖所有重试尝试和退避等待。预算耗尽时，即使还有剩余重试次数，请求也会被终止。

### backendRequest

单次后端请求的时间预算：从网关开始发送请求，到收到完整响应。配置了重试时，每次尝试都有自己的 `backendRequest` 预算，而总时间由 `request` 约束。

## 特殊值与行为

| 值 | 含义 |
|----|------|
| `"0s"` | 禁用超时（无限制）——适用于长连接 streaming |
| 省略 | 回退到 `EdgionGatewayConfig` 中的全局默认值 |
| 正值时长 | 显式限制 |

### backendRequest 被 request 约束

`backendRequest` 不得超过 `request`。如果配置的 `backendRequest` 大于 `request`，它会被自动截断到 `request`，确保单次尝试不会超出端到端预算。

### 与重试的交互

由于 `request` 约束包含重试在内的整个事务，请将其设置得足够大，以允许配置的重试 `attempts`（加上退避等待）完成。`backendRequest` 约束每次单独尝试。详见 [重试策略](./retry.md)。

## 示例

### 示例 1：标准 API 超时

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: timeout-api
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api
      timeouts:
        request: "60s"
        backendRequest: "15s"
      backendRefs:
        - name: api-service
          port: 8080
```

### 示例 2：长连接 streaming（禁用请求超时）

```yaml
rules:
  - matches:
      - path:
          type: PathPrefix
          value: /stream
    timeouts:
      # streaming 连接不设端到端时间限制
      request: "0s"
      backendRequest: "30s"
    backendRefs:
      - name: streaming-service
        port: 8080
```

### 示例 3：超时与重试结合

```yaml
rules:
  - timeouts:
      request: "30s"
      backendRequest: "8s"
    retry:
      attempts: 3
      backoff: "200ms"
      codes:
        - 503
    backendRefs:
      - name: api-service
        port: 8080
```

每次尝试受 `backendRequest`（8s）约束；所有尝试加上退避的总时间受 `request`（30s）约束。

## 相关文档

- [重试策略](./retry.md)
- [会话保持](./session-persistence.md)
