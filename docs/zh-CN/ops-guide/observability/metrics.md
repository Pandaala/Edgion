# 监控指标

> **🔌 Edgion 扩展**
>
> `edgion_*` 指标名称和可配置的 `backend_requests` 标签集是 Edgion 特有功能，不属于标准 Gateway API 规范。

Edgion 从 Gateway（数据面）、Controller（控制器）和 Center（联邦中心）进程暴露 Prometheus 指标。指标面故意保持精简：Gateway 数据面只使用计数器和 Gauge（加一个可选的 Histogram），高基数的单请求问题由 [access log](./access-log.md) 而非指标标签来回答。

## 概述

每个进程以 Prometheus 文本格式暴露 `/metrics` 端点，均使用独立监听器。Controller 和 Center 的 `/metrics` 位于各自专用的 metrics 端口（`12190` / `12290`），与管理/API 端口分离——`/metrics` 不再挂载在管理端口上。

| 进程 | 默认端点 | 路径 | 认证 |
|------|---------|------|------|
| Gateway | `:12090` | `/metrics` | 无（独立监听器） |
| Controller | `:12190` | `/metrics` | 无（独立监听器，与 Admin 端口 `12101` 分离） |
| Center | `:12290` | `/metrics` | 无（独立监听器，与 Admin 端口 `12201` 分离） |

`/metrics` 故意跳过认证，遵循 Kubernetes controller-manager 惯例。输出仅包含聚合数字——不含请求体，默认情况下不含资源名称（见下方基数说明）。

## 抓取配置

### Prometheus Operator（ServiceMonitor）

```yaml
apiVersion: monitoring.coreos.com/v1
kind: ServiceMonitor
metadata:
  name: edgion-gateway
  namespace: monitoring
spec:
  selector:
    matchLabels:
      app: edgion-gateway
  namespaceSelector:
    matchNames:
      - gateway-system
  endpoints:
    - port: metrics
      interval: 15s
      path: /metrics
```

### 原生 Prometheus 抓取配置

```yaml
scrape_configs:
  - job_name: edgion-gateway
    metrics_path: /metrics
    static_configs:
      - targets: ["10.0.0.10:12090"]
  - job_name: edgion-controller
    metrics_path: /metrics
    static_configs:
      - targets: ["10.0.0.11:12190"]   # 独立指标监听器，非 Admin 端口
  - job_name: edgion-center
    metrics_path: /metrics
    static_configs:
      - targets: ["10.0.0.12:12290"]   # 独立指标监听器，非 Admin 端口
```

## Gateway 指标

| 指标 | 类型 | 标签 | 说明 |
|------|------|------|------|
| `edgion_backend_requests_total` | Counter | 可配置（见下文） | 唯一的单请求计数器。网关级总量和失败数通过 `sum(...)` 和 `status` 标签派生 |
| `edgion_ctx_active` | Gauge | none | 当前进行中的请求上下文数 |
| `edgion_gateway_request_duration_ms` | Histogram | none | 端到端请求延迟（数据面唯一的 Histogram），**默认禁用** |
| `edgion_mirror_requests_total` | Counter | `status` | 请求镜像结果。`status` ∈ `ok` / `channel_full` / `connect_err` / `connect_timeout` / `write_err` / `timeout` |
| `edgion_rr_selector_truncated_total` | Counter | none | RoundRobin 选择器截断事件（后端数量超过 `u16::MAX`），用于告警配置异常 |
| `edgion_link_sys_es_bulk_channel_full_total` | Counter | none | 提交时 Elasticsearch 生产者→批量循环通道已满；日志发送路径的持续反压信号 |

### `edgion_backend_requests_total` 标签

此计数器的标签完全由 `metrics.backend_requests.labels` 配置控制，可在详细程度和基数之间权衡。默认值较为保守：

```yaml
metrics:
  backend_requests:
    labels:
      # 以下每个开关控制一个 namespace+name 对（或单个枚举）
      gateway: true     # gateway_namespace + gateway_name
      route: false      # route_namespace + route_name
      backend: false    # backend_namespace + backend_name
      protocol: true    # 有界枚举：http / grpc / websocket
      status: true      # 有界枚举：2xx / 3xx / 4xx / 5xx / failed
```

| 开关 | 添加的标签 | 默认值 | 说明 |
|------|-----------|--------|------|
| `gateway` | `gateway_namespace`、`gateway_name` | `true` | Gateway 资源标识 |
| `route` | `route_namespace`、`route_name` | `false` | 启用可获取单路由流量；增加基数 |
| `backend` | `backend_namespace`、`backend_name` | `false` | 启用可获取单后端错误率；增加基数 |
| `protocol` | `protocol` | `true` | 有界枚举，基数成本几乎为零 |
| `status` | `status` | `true` | 有界枚举，基数成本几乎为零 |

> 更改这些开关需要 Gateway **重启**——标签集在指标首次注册时固定。

⚠️ `test_key` / `test_data` 标签**仅**在 `--integration-testing-mode` 模式下且 Gateway 注解显式开启时出现。`test_data` 每请求变化，基数无界。**永远不要在生产中启用集成测试模式。**

### 启用延迟 Histogram

`edgion_gateway_request_duration_ms` 默认关闭。在 `metrics.request_duration` 段启用：

```yaml
metrics:
  request_duration:
    enabled: true
```

与 `backend_requests` 类似，Histogram 的标签可以在 `metrics.request_duration.labels` 下单独切换（gateway / route / backend 的 namespace+name、protocol、status），禁用的标签完全省略而非写入空字符串。保持标签集精简——每个活跃标签都会成倍增加 Histogram 系列数。

## Controller 指标

Controller 在独立的指标监听器（`:12190`，与 Admin 端口 `:12101` 分离）上以 `edgion_controller_*` 前缀暴露控制面指标。

| 指标 | 类型 | 标签 | 说明 |
|------|------|------|------|
| `edgion_controller_active` | Gauge | none | 活跃的资源控制器数量 |
| `edgion_controller_leader` | Gauge | none | Leader 选举状态（1 = leader，0 = standby） |
| `edgion_controller_resources_watched` | Gauge | `kind` | 按 kind 分类的当前被监控资源数 |
| `edgion_controller_init_sync_duration_seconds_last` | Gauge | `kind` | 按 kind 分类的最近一次初始同步时长 |
| `edgion_controller_reload_total` | Counter | none | 触发的总重载次数 |
| `edgion_controller_reload_duration_seconds_last` | Gauge | none | 最近一次重载时长 |
| `edgion_controller_reload_client_notifications_total` | Counter | none | 重载发送的客户端通知数 |
| `edgion_controller_status_writes_total` | Counter | `kind`、`backend`、`result` | 状态写入次数（`backend` ∈ kubernetes/filesystem，`result` ∈ success/error） |
| `edgion_controller_status_writes_skipped_total` | Counter | `kind` | 语义差异检查后跳过的状态写入次数 |
| `edgion_controller_unexpected_apply_during_init_total` | Counter | `kind` | 初始化期间收到的 Apply 事件（应保持为 0；非零表示 watcher 流排序异常） |
| `edgion_controller_resource_abnormal` | Gauge | `kind`、`namespace`、`name`、`err`、`warn` | 至少有一个失败状态条件的每个资源的一个序列。`err` / `warn` 是触发条件的 JSON 数组；该严重级别无条件时为空字符串 |

`edgion_controller_resource_abnormal` 快照定时重建，默认上限 1000 条，以控制基数。用于对卡在异常状态的资源告警，如 `edgion_controller_resource_abnormal{err!=""}`。

## 联邦指标（Center 和 Controller）

联邦基线指标以 `edgion_fed_*` 前缀暴露。所有标签值域均为固定枚举。

| 指标 | 类型 | 标签 | 进程 |
|------|------|------|------|
| `edgion_fed_connections_active` | Gauge | `role`（center/controller） | 两者 |
| `edgion_fed_connection_events_total` | Counter | `role`、`event`（connected/disconnected/reload） | 两者 |
| `edgion_fed_connection_duration_seconds_last` | Gauge | `role` | 两者 |
| `edgion_fed_watch_events_total` | Counter | `kind`、`direction`（sent/recv） | 两者 |
| `edgion_fed_watch_list_total` | Counter | `kind`、`result` | Center |
| `edgion_fed_watch_errors_total` | Counter | `kind`、`reason` | Center |
| `edgion_fed_mark_offline_total` | Counter | `reason` | Center |
| `edgion_fed_peer_identity_check_total` | Counter | `result` | Center |
| `edgion_fed_aggregator_controllers` | Gauge | `cluster` | Center |
| `edgion_fed_aggregator_keys_total` | Gauge | `kind` | Center |
| `edgion_fed_fanout_total` | Counter | `op`、`result` | Center |
| `edgion_fed_ready_gate_wait_seconds_last` | Gauge | none | Controller |

## 基数

⚠️ 最重要的基数控制杠杆是 `edgion_backend_requests_total` 标签集。

使用默认值（单个 Gateway），计数器产生约 **15 个序列**（1 个 gateway × 3 种协议 × 5 种状态）。启用更多维度会成倍增加：

| 配置 | 大约序列数 |
|------|-----------|
| 默认 | 15 |
| `+ backend` | 15 × N_backend |
| `+ backend + route` | 15 × N_backend × N_route |

建议：从默认值加 `backend: true` 开始，观察 `{__name__="edgion_backend_requests_total"}` 的实时序列数，仅在了解成本后才启用 `route: true`。对于单请求详情，使用 [access log](./access-log.md)——不要通过扩展标签来解决。

## 常见场景

### 1. 集群错误率告警

`edgion_backend_requests_total` 默认携带 `status`，因此错误率告警无需额外标签：

```text
sum(rate(edgion_backend_requests_total{status=~"5xx|failed"}[5m]))
  / sum(rate(edgion_backend_requests_total[5m]))
```

### 2. 单后端错误率

启用 `backend` 标签，重启 Gateway，然后按后端查询：

```yaml
metrics:
  backend_requests:
    labels:
      backend: true
```

```text
sum by (backend_name) (rate(edgion_backend_requests_total{status="5xx"}[5m]))
```

### 3. 延迟 SLO

启用 Histogram（见上文场景），然后计算百分位数：

```text
histogram_quantile(0.99, sum(rate(edgion_gateway_request_duration_ms_bucket[5m])) by (le))
```

## 常见问题

### Q1：为什么 `/metrics` 中缺少 `edgion_gateway_request_duration_ms`？

**A**：默认禁用。设置 `metrics.request_duration.enabled: true` 并重启 Gateway。在此之前，通过聚合 access log 的时序字段（`ht`、`et`）来推算延迟。

### Q2：我在 `backend_requests.labels` 下添加了标签，但它没有出现。为什么？

**A**：标签集在计数器首次注册时固定，因此更改开关后必须重启 Gateway。热重载不会改变指标的标签。

### Q3：没有"总请求数"或"失败请求数"计数器。如何获取？

**A**：两者都从 `edgion_backend_requests_total` 派生。总请求数为 `sum(edgion_backend_requests_total)`；失败数为 `sum(edgion_backend_requests_total{status=~"5xx|failed"})`。Edgion 故意避免冗余计数器。

### Q4：如何检测卡在异常状态的资源？

**A**：抓取 Controller 的 `/metrics`，并对 `edgion_controller_resource_abnormal{err!=""}` 告警。每个序列通过 `kind` / `namespace` / `name` 标识一个资源，失败条件在 `err` / `warn` 标签中。

### Q5：`/metrics` 无需认证暴露是否安全？

**A**：端点只返回聚合数字——不含请求体，默认设置下不含资源名称。如有需要，在集群级别限制网络访问，但数据本身不敏感。

## 获取帮助

如遇问题：

1. 检查本文档的常见问题部分
2. 检查网关和 Controller 日志
3. 在 GitHub 提交 issue

## 相关文档

- [Access Log](./access-log.md)
- [Gateway 配置](../gateway/README.md)
- [edgion-cli](../edgion-cli.md)
