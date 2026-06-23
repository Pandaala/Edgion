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
| `codes` | Array\<Integer\> | 无 | 触发重试的 HTTP 状态码（400–599），与 Gateway API HTTPRouteRetry 的取值范围一致。超出范围的状态码被忽略。省略时仅重试连接错误 |

## 重试行为

### 触发条件

- **连接错误**：只要重试策略生效，连接错误就会触发重试（连接失败、在收到任何响应前被重置）。
- **状态码**：只有列在 `codes` 中的状态码才会触发重试。省略 `codes` 时，状态码重试关闭，只重试连接错误。
- **单次尝试超时（per-try timeout）**：配置了重试策略时，超过 `timeouts.backendRequest` 的尝试会被取消并重试（GEP-1731 per-try 超时语义）。未配置重试策略时，超时直接以 504 返回给客户端（详见[下文](#通过-backendrequest-实现单次尝试超时)）。
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

`timeouts.request` 的值约束**整个**请求的时间，包括所有重试和退避等待。如果累计时间超过 `request`，则无论剩余重试次数如何，请求都会被终止。请将 `request` 设置得足够大，以允许配置的重试次数完成。详见 [超时配置](./timeouts.md)；`backendRequest` 与重试的交互见下方 [通过 `backendRequest` 实现单次尝试超时](#通过-backendrequest-实现单次尝试超时)。

### 通过 `backendRequest` 实现单次尝试超时

依据 Gateway API [GEP-1731](https://gateway-api.sigs.k8s.io/geps/gep-1731/)，`timeouts.backendRequest` 充当**单次尝试超时（per-try timeout）**：每次尝试（包括首次）都获得一个全新的 `backendRequest` 时间窗口，而 `timeouts.request` 封顶所有尝试的总时间。某次尝试在 `backendRequest` 内未收到响应时会被取消并重试，前提是 `attempts` 预算和剩余的 `request` 预算允许。

> **行为变更**（自本版本起）：配置了重试策略的路由会在后端请求超时后重试——包括依赖全局默认后端超时的路由。此前 backendRequest 超时总是直接以 504 返回，不重试。

```yaml
rules:
  - retry:
      attempts: 2
    timeouts:
      request: "10s"        # 所有尝试的总预算
      backendRequest: "1s"  # 每次尝试最多 1s
    backendRefs:
      - name: api-service
        port: 8080
```

注意事项：

- 由 `request` 截止时间或客户端 `grpc-timeout` 引发的超时**不会**重试——调用方已经没有时间预算了。
- 规则未设置 `backendRequest` 时，全局默认后端超时（`EdgionGatewayConfig.defaultRequestTimeout`，默认 `60s`）生效，并且同样作为单次尝试的上限——因此**任何**配置了重试策略的规则都会获得超时重试行为。配置重试策略时请务必同时设置 `timeouts.request`：否则没有端到端上限，最坏情况为 `(1 + attempts) × backendRequest`（全默认配置下为 4 × 60s）。
- 不存在"只按状态码重试、超时不重试"的模式：配置任何重试策略（哪怕只配 `codes`）都会让该规则获得超时重试——这是 GEP-1731 的要求，它把单次尝试超时归入连接错误一类。
- 只有在尚未向客户端发送任何内容时，单次尝试超时才会重试。一旦响应头已转发到下游，传输响应体过程中的超时会直接终止请求而不是重试。
- 重试会重新执行常规负载均衡，**不会**排除刚刚超时的后端。RoundRobin 通常会轮转到其他端点，但 ConsistentHash 会确定性地再次选中**同一个**后端（哈希键相同）；在 ConsistentHash 路由上使用较紧的单次尝试超时时，请配合熔断器 annotation，让持续缓慢的端点被摘除。
- 当剩余的 `request`/`grpc-timeout` 预算小于计算出的 `backoff` 延迟时，请求**不会**被重试：遵守配置的最小退避时间会达到或超过截止时间，因此依据 GEP-1731（禁止在配置的最小时长之前重试），网关会以绑定截止时间的超时直接失败请求，而不是发起一次低于最小退避的立即重试。若希望最后一次尝试得以执行，请将 `timeouts.request` 设置得足以容纳至少一个完整的 `backoff` 间隔。
- 每次被取消并重试的尝试都会在访问日志错误链中记录 `BackendRequestTimeoutRetry`（504_03），且该尝试的 per-upstream 日志条目会被标记 504 状态与超时错误类型。
- 每个由 Edgion 做出的重试决策都会在该次尝试的访问日志 `upstreams[].err` 字段中显式打标：连接失败携带错误类型（如 `ConnectTimedout`），单次尝试超时携带超时类型，状态码重试携带 `RetryOnStatus`，过期 keepalive 连接重试携带 `ConnectionReuseRetry`（由 Pingora 内部自行决定的重试，如 h2 GOAWAY 重试，可能表现为一条额外的无标签条目）。因此按原因统计的重试率完全可以通过访问日志聚合得到；项目有意不提供按重试计数的 Prometheus 指标（参见指标设计规则）。
- 如果客户端在响应送达之前就断开连接，请求级 `errors` 数组会记录 `ClientClosed`（常见情况下状态码为 499；投递失败的记录会保留原有的错误状态码——请按 `errors` 字段过滤，而非按状态码）。在 HTTP/1.1 上，进行中的尝试会在毫秒级内被中止；一条带有多个 `upstreams` 条目的 `ClientClosed` 记录可以量化花在已离开调用方身上的重试开销。
- 精度说明：Edgion 将 `backendRequest` 实现为逐 I/O 操作的读/写超时，因此持续有进展的尝试不会在传输中途被杀掉（在响应头阶段最接近 Envoy 的 `per_try_timeout`，在响应体传输阶段最接近 `per_try_idle_timeout`）。特别地，对于带长生命周期流的 gRPC 路由，过紧的 `backendRequest` 与重试策略组合会切断空闲的流——请按预期最长的静默期来设置该值。

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
