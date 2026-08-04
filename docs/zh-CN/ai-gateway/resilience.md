# 弹性与故障切换

AI 弹性由 HTTPRoute 重试许可、每个 EdgionAIService 内的凭据优先轮次、Route 级后端故障切换，
以及可选的 EdgionBackendTrafficPolicy（EBTP）目标控制共同组成。

## 共享尝试上限

`HTTPRoute.retry.attempts` 表示重试次数，因此总尝试次数为 `1 + attempts`。Edgion 最多支持
**7 次重试 / 8 次总尝试**。超过该上限的 Route 会变为 `Accepted=False`，原因为
`UnsupportedValue`。Route 重试、提供商故障切换与凭据轮转共用这一上限。

## 凭据优先轮次

发生可重试失败时：

1. 优先换同一 EdgionAIService 内的其他可用凭据。
2. 仅当所选 Service 没有可用凭据时，再通过 `backendSelection` 换到其他 Route 后端。
3. 若当前所有可用目标都已扫描且仍有重试容量，且上一轮至少有一次已派发的可重试失败，则可开启新一轮。

派发前跳过（无可用凭据、模型不匹配、饱和）既不消耗 HTTPRoute 重试，也不计入 retry-budget。
每次实际派发会计入所选 AI Service 的 retry-budget 分母；可重试的已派发失败消耗一次 HTTPRoute
重试，并计入该源 Service 的 retry-budget 分子。

## SSE / 响应承诺

一旦 Edgion 开始向客户端流式写入响应体，就不会再为该请求切换到其他提供商或凭据。流中途提供商
失败会表现为流错误，而不是静默换后端。

## HTTPRoute 重试门闩

重试准入仍需满足：

- 匹配 `HTTPRoute.retry.codes`（在响应转换前按提供商 HTTP 状态判断）
- 剩余尝试预算
- 失败源 Service 上已配置的 EBTP `retryConstraint` 聚合预算
- 既有的非幂等重放规则（需要时通过 `edgion.io/retry-non-idempotent=true` 显式开启）

派发前的建连失败仍可走常规连接重试路径。

## 面向 AI 的 EdgionBackendTrafficPolicy

EBTP 直接指向同 Namespace 的 `edgion.io/EdgionAIService`。它不挂到 HTTPRoute，也不定义后端列表。

| 段 | 对 EdgionAIService |
|----|--------------------|
| `connection` | 支持 — 拨号 / 建连超时 |
| `retryConstraint` | 支持 — 聚合防风暴预算 |
| `circuitBreaker` | 支持 — Service 级并发上限 |
| `outlierDetection` | 支持 — 被动驱逐，按凭据隔离 |
| `loadBalancer` / `healthCheck` / `upstreamAuthority` | 拒绝（fail-close） |

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: ai-provider-resilience
  namespace: production
spec:
  targetRefs:
    - group: edgion.io
      kind: EdgionAIService
      name: primary-provider
  connection:
    connectTimeout: 2s
  retryConstraint:
    budget:
      percent: 20
      interval: 10s
  circuitBreaker:
    maxParallelRequests: 200
  outlierDetection:
    consecutiveErrors: 5
    consecutiveGatewayErrors: 3
    consecutiveLocalOriginFailures: 3
    ejectionTime: 30s
    maxEjectionTime: 5m
```

对 AI 目标，outlier 状态按凭据别名隔离，因此一个失败或被限流的凭据不会驱逐兄弟凭据。驱逐到期后，
half-open 租约会放行一次探测请求；其他请求在探测成功前继续绕过该目标。

`retryConstraint: {}` 启用默认值。缺省则关闭聚合门闩。HTTPRoute retry 仍决定请求是否可重试；
EBTP 只限制聚合重试量。

## 相关

- [路由与选择](./routing.md)
- [凭据与限额](./credentials-and-limits.md)
- [HTTPRoute 重试](../user-guide/http-route/resilience/retry.md)
- [熔断](../user-guide/http-route/backends/circuit-breaker.md)
