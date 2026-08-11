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
| `edgion_mirror_requests_total` | Counter | `status` | 请求镜像结果。`status` ∈ `ok` / `body_abandoned` / `channel_full` / `connect_err` / `connect_timeout` / `write_err` / `timeout`；`body_abandoned` 表示镜像停止前下游传输没有到达真实 EOS，Edgion 主动取消镜像，避免镜像目标把不完整上传当成完整 body。只有请求头携带传输层 EOS 才能证明空 body 已完整；HTTP/2 中仅有 `Content-Length: 0` 声明并不能提供该保证。镜像缓冲区满则是独立的 `channel_full` |
| `edgion_rr_selector_truncated_total` | Counter | none | RoundRobin 选择器截断事件（后端数量超过 `u16::MAX`），用于告警配置异常 |
| `edgion_link_sys_es_bulk_channel_full_total` | Counter | none | 提交时 Elasticsearch 生产者→批量循环通道已满；日志发送路径的持续反压信号 |
| `edgion_outbound_template_fallback_total` | Counter | `field`（`timeout_ms`） | webhook / ForwardAuth 的 `timeoutMsTemplate` 渲染失败并回退到静态字段。详见[连接器 → Templates（模板）](../infrastructure/connectors.md#模板-templates) |
| `edgion_outbound_success_predicate_fail_total` | Counter | 无 | webhook / ForwardAuth 响应的状态码已被接受，但 `success.body` 断言将其判定为失败（"200 OK 但 body 里有错误"的情形）。详见[连接器 → Success（成功判定）](../infrastructure/connectors.md#成功判定-success) |

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

### 可选的 GenAI Token 计数器

`edgion_gateway_genai_tokens_total` 默认禁用。需要用 Prometheus 聚合 Token
速率或趋势时，可在 Gateway 进程配置中启用：

```yaml
metrics:
  genai_tokens:
    enabled: true
```

这是启动期配置；修改后必须重启 Gateway。禁用时不会注册该指标族。计数器只有三个
固定枚举标签：

- `direction`：`input` 或 `output`
- `provider`：`openai`、`openai_compatible` 或 `anthropic`
- `usage_source`：`reported` 或 `estimated`

三者的笛卡尔积使每个 Gateway 进程最多产生十二条序列。指标刻意不包含 `model`
标签，因此模型名称不会扩大 Prometheus 基数。应使用
`rate(edgion_gateway_genai_tokens_total[5m])` 观察速率，不要把原始计数器视为持久状态：
Gateway 进程每次重启都会重置计数器。

启用后，该计数器会聚合每一次已派发尝试（含随后重试）的上报或有界估算用量；派发前失败不计入。
它不导出资源、模型、凭据或请求身份标签。

该指标**不是**持久化用量或计费账本。独立于指标开关，access log 会记录最终请求汇总以及有界的
`genai.attempts` 账本（含每次尝试的用量来源、token、成本、状态/失败、Retry-After 与 TTFT）。
歧义结果可能为保护限流而保留完整的「输入 + 最大输出」Redis 预留，但其持久日志只记录观测到或
有界估算的用量；保留的预留不等于已声明消耗。需要持久核算时，应投递并保留 access log；
不要把 Prometheus 计数器值作为权威计费记录。

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
| `edgion_controller_leader_job_alive` | Gauge | `leader_job`（`acme` \| `conf_sync_rotation` \| `dns_resolver`） | Leader-job worker 的存活状态：`1` = 正在运行，`0` = 已停止或死亡。标签刻意使用 `leader_job` 而非 `job`——Prometheus 把 `job` 保留作为 target 标签，在默认 `honor_labels: false` 下会把导出的 `job` 标签重命名为 `exported_job`，从而破坏按文档名称编写的告警 |

`edgion_controller_resource_abnormal` 快照定时重建，默认上限 1000 条，以控制基数。用于对卡在异常状态的资源告警，如 `edgion_controller_resource_abnormal{err!=""}`。

### Leader-job 死亡告警

`edgion_controller_leader_job_alive` 为**每个副本上曾经启动过的每个 leader-job worker**各生成一个序列——
对于从未在该副本上启动过的 job，序列**不存在**，而不是 `0`（例如 `conf_sync_security.auto_ca` 关闭时，
该副本上永远不会启动 `conf_sync_rotation` worker，因此完全不会产生 `{leader_job="conf_sync_rotation"}`
这个序列；序列不存在可以正确抑制告警，而不是对一个本就不该运行的 job 无休止地报警）。

```promql
edgion_controller_leader_job_alive == 0
  and on(instance) edgion_controller_leader == 1
```

告警配置 `for: 2m`——该 Gauge 在 worker 每次正常的 backoff-restart 迭代期间都会短暂降到 `0`，窗口太短会对
常规重启报警，而不是真正死亡的 worker。`on(instance) edgion_controller_leader == 1` 把告警范围限定在当前
持有 leadership 的副本上——非 leader 副本上的 `0`（job 从未在那里启动）是预期状态，不应报警。`and` 保留了
左侧的 `leader_job` 标签，使告警输出能直接指明是哪个 job 死亡。

按设计，**不会自动重启**死亡的 leader-job worker（panic 通常意味着状态已损坏，盲目重启有陷入 panic 循环
的风险）——恢复方式是告警 + 运维介入（`POST /api/v1/reload` 或重启 Pod）。另外，如果控制面 lifecycle 本身
异常终止，`/health` 现在会返回 `503`（这是一个 deadman 机制，与本节这个逐 worker 的 Gauge 是不同的信号）——
完整的 lifecycle 恢复设计参见
[Supervision & recovery](https://github.com/Pandaala/Edgion/blob/main/skills/01-architecture/01-controller/03-config-center/02-kubernetes/00-lifecycle.md#supervision--recovery)。

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

## 系统降级状态（System error state）

`edgion_system_error_state` 是一个用于登记**"进程还在运行，但处于已知降级状态"**的 Gauge。Gateway 和
Controller 两侧都会发射它（所以它在本文单列一节），条件生效期间为 `1`，否则为 `0` 或不存在。

| 指标 | 类型 | 标签 | 进程 |
|------|------|------|------|
| `edgion_system_error_state` | Gauge | `component`、`reason` | 两者 |

两个标签都取自封闭集合，因此整个指标族在每个 Pod 上只有个位数条 series。`component` / `reason`
取值的权威清单在
[`edgion-common/src/observe/system_metrics.rs`](https://github.com/Pandaala/Edgion/blob/main/edgion-common/src/observe/system_metrics.rs)，
每个 reason 的文档注释说明了它由什么触发、以及是否会被清零。

这里有两类 reason，区别决定了你的告警怎么写：

- **锁存型（latching）**——启动期的配置决策（如 `skip_tls`、`cert_watch_failed`）。置位后不再清零，
  只有重启才能复位。直接对 `== 1` 告警即可。
- **瞬态型（transient）**——由同一进程置位、并在恢复时自行清零的运行时状态（如
  `confsync_disconnected`、`cert_rotation_failed`）。必须对**持续**的 `== 1` 告警并加 `for:` 约束——
  单次闪断是正常现象，对它告警只会制造噪音。

完整的 PAGE / TICKET / 仅日志分层（含推荐的 `for:` 时长及每一条的取舍理由）维护在
[conf_sync 运维参考](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/02-config/00-controller-config.md)
的 **Alerting** 小节。那里是唯一权威来源，本文有意不复述。

### 某个 Gateway 还在和 Controller 同步吗？

这个指标族里最值得配上的一条查询：

```promql
edgion_system_error_state{component="conf_sync_client",reason="confsync_disconnected"} == 1
```

要从 **Gateway** Pod（`:12090`）抓取，而不是 Controller——每个 Gateway 在自己的 registry 上置位这个
标记，所以**坏掉的那个 Pod 就是报告它的那个 Pod**。

持续 1 超过约 10 分钟，说明那个 Gateway 的配置同步链路已断且未恢复。这**不是**流量故障：Gateway 会
继续用最后一份已知配置提供服务，这是刻意设计——控制面抖动绝不该拿数据面可用性去换。它真正意味着这个
Gateway 的配置已经**冻结**：从那一刻起，所有路由、后端、证书变更都会静默地到不了它，于是一次发布可以
"成功"，而那个 Pod 仍在服务旧世界。没有别的信号能暴露这一点：该 Pod 会一直是 `Ready`（readiness 按设计
在启动时锁存一次），也会一直返回 200。

用约 10 分钟而不是更短的窗口：Controller 滚动重启会合法地让这个 gauge 置 1 几秒，而 Gateway 自身的重连
退避上限是 30 秒。

适用范围：它跟踪的是 Gateway 的 `WatchServerMeta` 流，这正是传输层故障（Controller 挂掉、证书过期、CA
不匹配）会打断的东西。仅限于某一种资源 kind 的 watch 的故障——例如某个 list 响应这个 Gateway 解析不
了——**不会**让它置位，那类问题只出现在 Gateway 的日志里。

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

### Q5：某个 Gateway 流量一切正常，但我的配置变更在它上面就是不生效，为什么？

**答**：多半是它到 Controller 的配置同步链路断了。检查那个 Gateway 自己的 `:12090` 上的
`edgion_system_error_state{component="conf_sync_client",reason="confsync_disconnected"}`——持续为
`1` 说明它一直没重连上，正在服务冻结的配置。注意这种状态下 `/ready` 和流量按设计都是绿的，所以这两个
都不会告诉你。参见[系统降级状态](#系统降级状态system-error-state)。

### Q6：`/metrics` 无需认证暴露是否安全？

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
