# 弹性与故障切换

AI 弹性由 HTTPRoute 重试许可、每个 EdgionBackend 内的凭据优先轮次、Route 级后端故障切换，
以及可选的 EdgionBackendTrafficPolicy（EBTP）目标控制共同组成。

## 共享尝试上限

`HTTPRoute.retry.attempts` 表示重试次数，因此总尝试次数为 `1 + attempts`。Edgion 最多支持
**7 次重试 / 8 次总尝试**。超过该上限的 rule 会被拒绝并从数据面丢弃；具体上报哪个状态条件见
[AI 规则拒绝](#ai-规则拒绝)。Route 重试、提供商故障切换与凭据轮转共用这一上限。

## AI 规则拒绝

AI 挂载错误——`retry.attempts` 超限、后端 kind 混用、`AiProxy` 缺失或重复、不支持的 backendRef
Filter——都是**规则级**（per-rule）问题。出问题的 rule 一定会从数据面丢弃；上报哪个状态条件取决于
还剩下什么：

| 情形 | 受影响 `parentRef` 上的条件 |
|------|------------------------------|
| HTTPRoute 仍有 rule 在服务 | `Accepted=True`，`PartiallyInvalid=True/RuleInvalid`，消息为 `rules[i]: <原因> (N of M rules active)` |
| HTTPRoute 已没有任何 rule 在服务 | `Accepted=False/UnsupportedValue`，列出每条 `rules[i]: <原因>`；`PartiallyInvalid` 不会为 `True`（之前的 `True` 会被清成 `False`） |

`Accepted=False` 会把整条 route 从该 parent 的网关路由表中移除，因此它只用于「反正也没有任何 rule
会服务」的情况。一条同时包含坏 AI rule 和正常非 AI rule 的 route，那些正常 rule 会继续服务。

**委派例外**：升级判断只看 route 自身的 `spec.rules`。位于**被委派子路由**规则中的 AI 违规会在
展平阶段被丢弃，并且始终以 `PartiallyInvalid=True` 上报，不会触发 `Accepted=False`——这与「被丢弃的
委派规则不拒绝其父路由」的通用规则一致。若该丢弃导致父路由一条 rule 都不剩，父路由会停止服务而
`Accepted` 仍为 `True`；此时请读 `PartiallyInvalid` 消息中的 `resolvedRules[i]: <原因>` 条目。

## 凭据优先轮次

发生可重试失败时：

1. 优先换同一 EdgionBackend 内的其他可用凭据。
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

EBTP 直接指向同 Namespace 的 `edgion.io/EdgionBackend`。它不挂到 HTTPRoute，也不定义后端列表。

| 段 | 对 EdgionBackend |
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
      kind: EdgionBackend
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

预算和最低重试额度使用毫秒精度的单调滑动窗口，系统时间调整不会改变窗口寿命，
早于上一窗口的流量不再参与计算。请求在重试期间保持已捕获的策略代际，策略更新也不会
替换该请求使用的预算。并发决策保留软上限：N 个并发决策者最多额外放行 N−1 次。

如果决策暂停太久，采样时刻所需的历史已被覆盖，则拒绝该次重试。计数不会回绕：
桶内任一计数器达到内部饱和值（2^40 − 1）后，在该历史过期前拒绝重试。
48 位窗口编号耗尽时，该预算实例也会拒绝重试。这些保护同时适用于 Service 和 AI 后端。

## 相关

- [路由与选择](./routing.md)
- [凭据与限额](./credentials-and-limits.md)
- [HTTPRoute 重试](../user-guide/http-route/resilience/retry.md)
- [熔断](../user-guide/http-route/backends/circuit-breaker.md)
