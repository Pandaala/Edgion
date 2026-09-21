# 熔断器（Circuit Breaker）

> **🔌 Edgion 扩展**
>
> 本功能通过 `EdgionBackendTrafficPolicy` 自定义资源的 `outlierDetection` 字段为后端启用被动逐出，属于 Edgion 扩展能力。

## 概述

熔断器（异常点检测）会在网关侧跟踪每个后端端点的连续失败次数，当达到阈值时自动逐出该端点，冷却到期后自动恢复。
该功能用于防止级联故障，保护上游服务，与主动健康检查互补。

**失败事件**拆分为三个独立计数器：连续**应用层** 5xx（任意 ≥500 且不是 502/503/504）、连续**上游网关**失败（后端返回 502/503/504）、连续**本地源**失败（连接失败/超时、TLS 握手超时、连接重置——网关根本没拿到可用的 HTTP 响应）。任意一个计数器达到阈值都会逐出该端点。
**成功事件**（非 5xx 响应）会重置全部连续失败计数。

异常点检测通过挂载到目标 `Service` 的 `EdgionBackendTrafficPolicy` 资源的 `spec.outlierDetection` 字段配置。同一个 `EdgionBackendTrafficPolicy` 还承载了同一 Service 的[主动健康检查](./health-check.md)与[负载均衡算法](../lb-algorithms.md)；三者的完整 schema 参考见 [full schema reference](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/20-edgion-backend-traffic-policy.md)（英文）。

## 快速开始

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: my-backend-policy
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: my-backend
  outlierDetection:
    consecutiveErrors: 5
    ejectionTime: 30s
    maxEjectionPercent: 50
---
apiVersion: v1
kind: Service
metadata:
  name: my-backend
  namespace: production
spec:
  ports:
    - port: 8080
      targetPort: 8080
```

## 配置参考

`spec.outlierDetection`：

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `consecutiveErrors` | Integer | `5` | 连续应用层 5xx 失败（任意 ≥500 且非 502/503/504）多少次后逐出，最小值 `1` |
| `consecutiveGatewayErrors` | Integer | 默认取 `consecutiveErrors` | 连续上游网关失败（后端返回 502/503/504）多少次后逐出 |
| `consecutiveLocalOriginFailures` | Integer | 默认取解析后的 `consecutiveGatewayErrors` | 连续本地源失败（连接/TLS 握手/连接重置/代理错误）多少次后逐出 |
| `ejectionTime` | Duration | `30s` | 逐出基础冷却时间（GEP-2257 时长），会按端点被逐出次数做退避放大。最小值 `1s` |
| `maxEjectionTime` | Duration | `max("5m", ejectionTime)` | 退避放大后的逐出时长上限 |
| `maxEjectionPercent` | Integer | `50` | 最大逐出比例。`0` 表示不限制；超过 `100` 会被拒绝 |

校验失败的 spec（例如 `consecutiveErrors: 0`、`ejectionTime: 500ms`（亚秒级时长会被拒绝）或 `maxEjectionPercent: 150`）会使整个 `EdgionBackendTrafficPolicy` 变为 `Accepted=False`；按照宽松准入策略，资源仍会同步，网关侧会跳过该无效 spec，对目标 Service 保持异常点检测禁用。

三个失败计数器相互独立——这与 Envoy 的 `splitExternalLocalOriginErrors` 思路一致，使得正在优雅 503 限流的后端不会与端口彻底不可达的后端使用同一阈值被逐出。限制：应用层返回的 502/503/504 会被计入网关失败（网关只能看到状态码）——如果你的应用会直接返回这些状态码，请相应提高 `consecutiveGatewayErrors`。对 UDPRoute 无效。

## 工作原理

1. 后端端点发生失败事件（属于三类中的某一类）→ 该类的连续失败计数 +1
2. 后端端点收到成功响应 → 全部连续失败计数归零
3. 任意一类的连续失败计数达到其阈值 → 端点被逐出
4. 逐出期间，负载均衡选择会跳过该端点
5. 经过退避放大后的逐出时长后 → 端点自动恢复，重新加入选择池

`maxEjectionPercent` 防止所有端点同时被逐出。例如设置 50%，2 个端点中最多逐出 1 个。

## 与重试的相互作用

异常点检测统计的是**上游尝试**，而不是客户端请求。每个完成的上游响应都会先被分类，然后路由级[重试](../resilience/retry.md)才决定是否重试，因此每一次被重试的尝试都会贡献自己的成功/失败样本。

也就是说，配置了 `retry.attempts: N` 的规则，一个客户端请求最多会发出 `N + 1` 次上游尝试，这一个请求最多可贡献 `N + 1` 次连续失败。每次尝试都会重新执行端点选择，样本落在负载均衡实际选中的端点上；当所有尝试都命中同一个端点时——单端点 Service，或被[严格模式会话保持](../resilience/session-persistence.md)固定的请求——`N + 1` 次失败会全部累积到该端点上。

调优要点：

- 若不希望单个客户端请求就能触发逐出，请把 `consecutiveErrors` / `consecutiveGatewayErrors` 设置为大于 `attempts + 1`。
- `consecutiveLocalOriginFailures` 需要同样处理，且对完全没有配置 `retry` 的规则同样适用：连接失败会按 `EdgionGatewayConfig.spec.retry.attempts`（默认 `2`）重试，因此默认情况下一个客户端请求最多可贡献 3 次本地源失败。
- `maxEjectionPercent` 无法保护单端点 Service——该上限的下界为 1 个端点，因此即便使用默认的 `maxEjectionPercent: 50`，Service 的唯一端点也始终可能被逐出。逐出期间网关没有可选的端点，会在逐出到期前返回本地 `503`。

## 调优建议

| 目标 | 调整方式 | 权衡 |
|------|----------|------|
| 更快响应故障端点 | 降低 `consecutiveErrors`（如 `3`） | 对瞬时抖动更敏感；短暂高峰也可能触发逐出 |
| 容忍偶发错误 | 提高 `consecutiveErrors`（如 `10`） | 移除真正故障端点的速度更慢 |
| 给恢复中的端点更多时间 | 提高 `ejectionTime`（如 `60s`） | 逐出期间容量降低时间更长 |
| 快速探测恢复 | 降低 `ejectionTime`（如 `10s`） | 仍有问题的端点会更早重新进入选择池，可能再次被逐出 |
| 保护小规模端点池的容量 | 降低 `maxEjectionPercent` | 若已达到上限，故障端点可能仍留在轮询中 |

大多数生产 Service 的推荐起点：`consecutiveErrors: 5`、`ejectionTime: 30s`、`maxEjectionPercent: 50`（即默认值）。结合下方指标进行调优。

⚠️ **在极小规模的端点池中**（1~2 个端点），`maxEjectionPercent` 几乎不提供保护——该上限的下界为 1 个端点，因此只要比例非 0，无论设得多低，单端点 Service 都始终可能被逐出。请结合规则的重试预算把连续失败阈值设得足够高（见[与重试的相互作用](#与重试的相互作用)），并同时配合主动健康检查，确保不健康端点在选择时仍被跳过。

## 与健康检查的关系

| | 健康检查 | 熔断器（异常点检测） |
|--|---------|--------|
| 方式 | 主动探测 | 被动统计 |
| 触发 | 定时发送探针 | 实际请求失败 |
| 恢复 | 探针成功 N 次 | 冷却到期自动恢复 |
| 配置 | `EdgionBackendTrafficPolicy` 的 `spec.healthCheck` | `EdgionBackendTrafficPolicy` 的 `spec.outlierDetection` |

两者位于同一个 `EdgionBackendTrafficPolicy` 资源上，可以同时使用，互补不冲突。

## 可观测性

熔断器暴露以下 Prometheus 指标：

| 指标 | 类型 | 说明 |
|------|------|------|
| `edgion_circuit_breaker_ejections_total` | Counter | 逐出事件总数（带 `class` 标签 = `gateway`/`application`/`local_origin`） |
| `edgion_circuit_breaker_ejected_endpoints` | Gauge | 当前逐出端点数 |
| `edgion_circuit_breaker_rejections_total` | Counter | 因逐出被拒绝的选择次数 |

所有指标带 `service` 标签（格式：`namespace/name`）。

## 相关文档

- [后端主动健康检查](./health-check.md)
- [负载均衡算法](../lb-algorithms.md)
- [Service 引用](./service-ref.md)
