---
name: metrics-features
description: Prometheus Metrics 端点和可用指标。
---

# Metrics

## 端点

| 端点 | 端口 | 说明 |
|------|------|------|
| Gateway Metrics | `:5901/metrics` | Prometheus 格式指标 |

## 指标规范

Edgion 的 Metrics 设计原则：

1. **无 Histogram**：避免高基数问题，只使用 Counter 和 Gauge
2. **Label 基数有限**：每个 Label 的值域 ≤ 数十个
3. **通过 GatewayMetrics struct 管理**：不直接使用 `metrics::counter!()` 宏
4. **Gauge 平衡**：所有 +1 路径都有对应的 -1

### 常见指标

| 指标名 | 类型 | 说明 |
|--------|------|------|
| `backend_requests_total` | Counter | 后端请求总数 |
| `active_connections` | Gauge | 活跃连接数 |

详细指标规范见 [../../03-coding/observability/01-metrics.md](../../03-coding/observability/01-metrics.md)。
