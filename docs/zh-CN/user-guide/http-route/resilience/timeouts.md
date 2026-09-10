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
| `request` | Duration | 无（不设端到端上限） | 从收到请求到返回响应的总时间，**包含所有重试** |
| `backendRequest` | Duration | EdgionGatewayConfig 的 `defaultRequestTimeout`（默认 `60s`） | 单次后端请求的超时时间（一次尝试，不含重试） |

两个字段均接受 Gateway API 时长字符串，如 `"500ms"`、`"10s"`、`"1m"`、`"1h"`。

## 超时类型说明

### request

整个事务的端到端时间预算，涵盖所有重试尝试和退避等待。预算耗尽时，即使还有剩余重试次数，请求也会被终止。

### backendRequest

单次后端请求的时间预算：从网关开始发送请求，到收到完整响应。当规则同时配置了重试策略时，`backendRequest` 充当单次尝试超时（per-try timeout）：每次尝试都有自己的 `backendRequest` 预算，超过该值的尝试会被取消并重试，总时间仍由 `request` 约束（详见 [重试策略 — 单次尝试超时](./retry.md#通过-backendrequest-实现单次尝试超时)）。

请求与响应 I/O 阶段的精度说明：Edgion 将这些预算实现为逐操作的读/写超时，持续有进展的尝试不会被中途切断；详见重试文档中的[精度说明](./retry.md#通过-backendrequest-实现单次尝试超时)。

### 连接建立

TCP 连接与上游 TLS 握手共享一个建连预算：取配置的连接超时、路由 `request`
剩余预算以及客户端 `grpc-timeout` 剩余预算中的最小值。连接超时优先使用选中
后端策略的 `connection.connectTimeout`，未配置时使用 `EdgionGatewayConfig`
的 `defaultConnectTimeout`。

例如连接超时为 5 秒，但请求仅剩 300 毫秒，停滞的 TCP 连接或 TLS 握手会在
约 300 毫秒后结束，实际时间受计时精度和调度影响。请求或 gRPC 预算耗尽后
不再重试，返回 HTTP 504 或 gRPC `DEADLINE_EXCEEDED`；两者均已过期时，
诊断优先归因于请求超时。

`backendRequest` 不缩短建连预算。禁用配置的连接超时也不会禁用仍有效的
请求或 gRPC 预算。此建连上限覆盖 TCP 与 TLS，不包含后续 HTTP/2 会话建立、
连接池取得过程，也不是流式响应的总时长上限。

## 特殊值与行为

| 值 | 含义 |
|----|------|
| `"0s"` | 禁用路由所配置的 deadline——适用于长连接 streaming；传输完全无进展时仍受协议级存活下限保护 |
| 省略 | `backendRequest` 回退到 `EdgionGatewayConfig` 的全局默认值（`defaultRequestTimeout`，默认 `60s`）；`request` **没有**全局默认——省略即不设端到端上限 |
| 正值时长 | 显式限制 |

对于发往 H2 upstream 的请求体，如果所有有效写 deadline 都被禁用，
Pingora 传输层仍保留 10 秒的逐次 capacity-wait 安全下限。它只在 upstream
完全不再授予请求体流控容量时触发；每次取得容量后都会重新计时，因此不是
streaming 总时长限制。任何正值的有效 `backendRequest`、`request` 或 gRPC
deadline 仍是权威写超时，无论它短于还是长于 10 秒。

### backendRequest 被 request 约束

`backendRequest` 不得超过 `request`。如果配置的 `backendRequest` 大于 `request`，它会被自动截断到 `request`，确保单次尝试不会超出端到端预算。

### 与重试的交互

由于 `request` 约束包含重试在内的整个事务，请将其设置得足够大，以允许配置的重试 `attempts`（加上退避等待）完成。`backendRequest` 约束每次单独尝试。详见 [重试策略 — 单次尝试超时](./retry.md#通过-backendrequest-实现单次尝试超时)。

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
