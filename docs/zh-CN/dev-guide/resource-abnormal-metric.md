# Controller 资源异常 Metric

Controller 暴露一条 Prometheus gauge `edgion_controller_resource_abnormal`，用于对所有写 status condition 的资源进行“当前异常”告警。

## Metric 定义

- 名称：`edgion_controller_resource_abnormal`
- 类型：gauge（值恒为 `1`；不存在 series 即代表正常）
- 采集路径：Controller 的 `/metrics` 端点（与 Prometheus recorder 的其他指标一起暴露）
- Label：
  - `kind`：资源 K8s `.kind` 字段（PascalCase，例如 `HTTPRoute`、`EdgionPlugins`）
  - `namespace`：资源命名空间（cluster-scoped 为空字符串）
  - `name`：资源名
  - `err`：JSON 数组字符串，记录所有 err 级触发 condition；空字符串表示该级别无触发
  - `warn`：JSON 数组字符串，记录所有 warn 级触发 condition；空字符串表示该级别无触发

err / warn 两个级别在同一资源上**共存**（各自是独立的 label），不再互相吞并。

## `err` / `warn` 划分规则

| 级别 | 触发 condition |
|------|---------------|
| `err` | `Accepted:False` / `ResolvedRefs:False` / `Programmed:False`；或 `Conflicted:True` / `ListenersNotValid:True`（反向语义） |
| `warn` | `Ready:False`；或 `PartiallyInvalid:True` |

每个 label 值是 JSON 数组：`[{"type":"...","status":"..."}, ...]`，按扫描顺序排列；若该级别无触发则 label 为空串 `""`。

示例：

```prometheus
edgion_controller_resource_abnormal{kind="HTTPRoute",namespace="default",name="bad",err="[{\"type\":\"ResolvedRefs\",\"status\":\"False\"}]",warn=""} 1
edgion_controller_resource_abnormal{kind="HTTPRoute",namespace="default",name="mixed",err="[{\"type\":\"Accepted\",\"status\":\"False\"}]",warn="[{\"type\":\"Ready\",\"status\":\"False\"}]"} 1
```

## 刷新机制

后台任务每 `tick_secs`（默认 `300`，最小 `30`）重建整份快照，原子替换：

- **异常出现** → 最多 `tick_secs + scrape 间隔` 后在 `/metrics` 可见
- **异常恢复或资源删除** → 最多 `tick_secs + scrape 间隔` 后 series 消失
- **Controller 重启** → 首次 tick 后重建（重启后短暂无该类 series）

系统有 **10 000 条** 的 snapshot hard cap，超出时会截断并打 `tracing::error!` —— 正常部署不会触及，仅作雪崩兜底。

## 配置

```yaml
# config/edgion-controller.yaml
metrics:
  resource_abnormal:
    enabled: true      # 默认 true；false 则不启动后台任务、/metrics 不输出这条 metric
    tick_secs: 300     # 默认 300；范围 [30, 3600]
```

`enabled = false` 时完全关闭：后台任务不 spawn，wrapper handler 不 append，零开销。

配置变更**需重启 Controller 生效**（不支持热 reload，和其它 Controller 配置一致）。

## 典型 Prometheus 告警规则

新版 label 是 JSON 数组字符串，告警规则按 `err!=""` / `warn!=""` 过滤：

```yaml
- alert: EdgionResourceError
  expr: edgion_controller_resource_abnormal{err!=""} == 1
  for: 5m
  labels:
    severity: critical
  annotations:
    summary: "Edgion {{ $labels.kind }} {{ $labels.namespace }}/{{ $labels.name }} is in error state"
    description: "err-level triggers: {{ $labels.err }}"

- alert: EdgionResourceWarn
  expr: edgion_controller_resource_abnormal{warn!=""} == 1
  for: 15m
  labels:
    severity: warning
  annotations:
    summary: "Edgion {{ $labels.kind }} {{ $labels.namespace }}/{{ $labels.name }} has warnings"
    description: "warn-level triggers: {{ $labels.warn }}"
```

建议 `for:` 设为 `tick_secs` 的 1-2 倍，避免单次 tick 抖动误报。

## 覆盖范围

自动覆盖所有“Controller 会写 status condition”的资源：Gateway、HTTPRoute、GRPCRoute、TCPRoute、TLSRoute、UDPRoute、EdgionPlugins、EdgionStreamPlugins、EdgionTls、EdgionAcme、EdgionGatewayConfig、BackendTLSPolicy、GatewayClass 等。

**不覆盖**：`Secret`、`ConfigMap`、`Service`、`EndpointSlice`、`Endpoint` 等 Controller 不写 condition 的资源。

## 运行时内部机制（给 operator / dev 读）

- 一个后台 `tokio::spawn` 任务每 `tick_secs` 轮询所有注册的 per-kind renderer
- 每个 renderer 扫 `RawStore<K>::snapshot()`，序列化为 JSON，递归找所有 `conditions[]`，`classify` 后产生 `Entry`（`err_conditions` + `warn_conditions` 两个列表）
- 所有 `Entry` 排序后 `ArcSwap::store` 原子替换当前快照
- `/metrics` 被 scrape 时读取当前快照并 append 到 common handler 输出后面
- 任意 renderer panic 被 `catch_unwind` 隔离，不影响其他 kind

## 相关源码

- `src/core/controller/observe/status_metrics.rs` — 主实现
- `src/core/common/config/mod.rs` — `MetricsConfig` / `ResourceAbnormalMetricsConfig`
- `src/core/controller/conf_mgr/conf_center/*/resource_controller.rs` — 各 backend 注册 renderer 的位置
