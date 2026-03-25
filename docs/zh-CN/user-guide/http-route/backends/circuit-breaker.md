# 熔断器（Circuit Breaker）

> **🔌 Edgion 扩展**
>
> 本功能通过 `edgion.io/circuit-breaker` Annotation 为后端启用被动逐出，属于 Edgion 扩展能力。

## 概述

熔断器会在网关侧跟踪每个后端端点的连续失败次数，当达到阈值时自动逐出该端点，冷却到期后自动恢复。
该功能用于防止级联故障，保护上游服务，与主动健康检查互补。

**失败事件**包括：连接失败、代理错误、上游返回 5xx 状态码。
**成功事件**（非 5xx 响应）会重置连续失败计数。

## 快速开始

在 `Service` 上添加注解即可启用：

```yaml
apiVersion: v1
kind: Service
metadata:
  name: my-backend
  namespace: default
  annotations:
    edgion.io/circuit-breaker: |
      consecutiveErrors: 5
      ejectionSeconds: 30
      maxEjectionPercent: 50
spec:
  ports:
    - port: 8080
      targetPort: 8080
```

## 配置参考

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `consecutiveErrors` | u32 | 5 | 连续失败次数后逐出（最小 1） |
| `ejectionSeconds` | u64 | 30 | 逐出冷却时间（秒，最小 1） |
| `maxEjectionPercent` | u32 | 50 | 最大逐出比例（0 = 不限制） |

## 工作原理

1. 后端端点发生失败事件 → 连续失败计数 +1
2. 后端端点收到成功响应 → 连续失败计数归零
3. 连续失败计数达到 `consecutiveErrors` → 端点被逐出
4. 逐出期间，负载均衡选择会跳过该端点
5. 经过 `ejectionSeconds` 秒后 → 端点自动恢复，重新加入选择池

`maxEjectionPercent` 防止所有端点同时被逐出。例如设置 50%，2 个端点中最多逐出 1 个。

## 与健康检查的关系

| | 健康检查 | 熔断器 |
|--|---------|--------|
| 方式 | 主动探测 | 被动统计 |
| 触发 | 定时发送探针 | 实际请求失败 |
| 恢复 | 探针成功 N 次 | 冷却到期自动恢复 |
| 配置 | `edgion.io/health-check` | `edgion.io/circuit-breaker` |

两者可以同时使用，互补不冲突。

## 可观测性

熔断器暴露以下 Prometheus 指标：

| 指标 | 类型 | 说明 |
|------|------|------|
| `edgion_circuit_breaker_ejections_total` | Counter | 逐出事件总数 |
| `edgion_circuit_breaker_ejected_endpoints` | Gauge | 当前逐出端点数 |
| `edgion_circuit_breaker_rejections_total` | Counter | 因逐出被拒绝的选择次数 |

所有指标带 `service` 标签（格式：`namespace/name`）。
