---
name: metrics-features
description: Prometheus Metrics 端点和可用指标。
---

# Metrics

## 端点

| 端点 | 端口 | 路径 | Auth | 说明 |
|------|------|------|------|------|
| Gateway Metrics | `:5901` | `/metrics` | 无 | 独立监听端口，`serve_with_shutdown` 启动 |
| Controller Metrics | Admin API（默认 `:8080`） | `/metrics` | 免 auth（unified_auth skip_paths） | 挂在现有 admin router 下 |
| Center Metrics | Admin API（默认 `:5810`） | `/metrics` | 免 auth（unified_auth skip_paths） | 挂在现有 router 下 |

> Prometheus scrape 按 K8s controller-manager 惯例免 auth。`/metrics` 内容为聚合数字，不包含资源名/请求体。

## 指标规范

Edgion 的 Metrics 设计原则：

1. **无 Histogram**：避免高基数问题，只使用 Counter 和 Gauge
2. **Label 基数有限**：每个 Label 的值域 ≤ 数十个
3. **通过 GatewayMetrics struct 管理**：不直接使用 `metrics::counter!()` 宏
4. **Gauge 平衡**：所有 +1 路径都有对应的 -1
5. **混合注册策略**：Hot 每请求路径（ctx_created / gateway_request_bytes 等）`Counter`/`Gauge` 直存、`new()` 急注册；Cold 失败/低频路径用 `OnceLock<Counter>/<Gauge>`，首次 record 才注册——未触发的指标不会出现在 `/metrics`，零值不占带宽

### 常见指标

| 指标名 | 类型 | 说明 |
|--------|------|------|
| `backend_requests_total` | Counter | 后端请求总数 |
| `active_connections` | Gauge | 活跃连接数 |
| `edgion_rr_selector_truncated_total` | Counter | RoundRobin 后端数 > `u16::MAX` 的截断事件（极罕见配置异常告警用，无 label） |
| `edgion_link_sys_es_send_total{result}` | Counter | Elasticsearch DataSender 每次 send 的结果，`result` ∈ `ok` / `failed_fallback` / `failed_no_cache` |

### Federation Metrics（Center + Controller）

Federation baseline metric 全部以 `edgion_fed_` 前缀暴露，定义与 helper 在 `src/core/common/observe/fed_metrics.rs`。所有 label 值域都是编译期常量（见同文件 `labels` 子模块）。

| 指标 | 类型 | Labels | 来源进程 |
|------|------|--------|---------|
| `edgion_fed_connections_active` | Gauge | `role` ∈ center/controller | 双 |
| `edgion_fed_connection_events_total` | Counter | `role`, `event` ∈ connected/disconnected/reload | 双 |
| `edgion_fed_connection_duration_seconds_last` | Gauge | `role` | 双 |
| `edgion_fed_watch_events_total` | Counter | `kind`, `direction` ∈ sent/recv | 双 |
| `edgion_fed_watch_list_total` | Counter | `kind`, `result` ∈ ok/version_too_old/parse_error/ready_wait | Center |
| `edgion_fed_watch_errors_total` | Counter | `kind`, `reason` ∈ parse_error/timeout/recv_error | Center |
| `edgion_fed_mark_offline_total` | Counter | `reason` ∈ heartbeat/disconnect/reload | Center |
| `edgion_fed_evict_stale_total` | Counter | `source` ∈ registry/aggregator | Center |
| `edgion_fed_session_reentry_total` | Counter | — | Center |
| `edgion_fed_aggregator_controllers` | Gauge | `cluster` | Center |
| `edgion_fed_aggregator_keys_total` | Gauge | `kind` | Center |
| `edgion_fed_consistency_mismatch_total` | Counter | — | Center |
| `edgion_fed_fanout_total` | Counter | `op` ∈ patch_enable/patch_profile, `result` ∈ ok/partial/fail | Center |
| `edgion_fed_ready_gate_wait_seconds_last` | Gauge | — | Controller |

> 注：原 ticket 的 `edgion_fed_stream_tx_drops_total` 暂未落盘——fed_sync 目前无 `try_send` 路径，补该 metric 需先引入 backpressure；未来工单再处理。

详细指标规范见 [../../03-coding/observability/01-metrics.md](../../03-coding/observability/01-metrics.md)。
