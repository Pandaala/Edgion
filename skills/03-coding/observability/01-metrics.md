# Metrics 规范

> Metrics 核心原则：Gateway **数据面**避免 metrics 爆炸（无 Histogram + 严控 Label 基数 + 统一 `GatewayMetrics` 管理），数据面异常不单独加 counter/tracing，走 access log (`AccessLogEntry.errors` + `EdgionStatus`) 统一承接；Controller / Center **控制面**同样主动克制新增 metric，控制面事件优先 `tracing::{info,warn,error}!` + status condition，只有"作为秒级告警阈值"或"Grafana 长期趋势核心 SLI"的场景才加 metric。细节观测 → access log / 结构化 tracing 日志聚合，不在 Prometheus 里扩指标。

## 核心原则

- **Gateway 数据面不引入 Histogram 类型**（无 `_bucket` / `_sum` / `_count` 三元组）
  - **作用域**：`src/core/gateway/` 下每请求 hot-path 的所有 metric。因为数据面每请求都走，histogram bucket × 多 label 的笛卡尔积极易爆炸（单条 histogram 发出 `N_bucket × 高基数` 条时序），Prometheus 存储和 scrape 都扛不住。
  - **唯一存量例外**：`edgion_gateway_request_duration_ms` — Gateway 端到端请求时延，核心 SLI（p50/p95/p99 是生产告警的主要依据）。Gateway 数据面仅此一条 histogram，通过 `MetricsConfig::request_duration` 配置项默认关闭、需显式 `enabled=true` 才写入，bucket 由 `set_buckets_for_metric` 显式注入（见 `gateway/observe/metrics/api.rs`），label 也是 per-label toggle 按需开启。
  - **如果要在 Gateway 数据面加第二条 histogram**：先改本条规则 + 附理由（为什么 counter/gauge 表达不出，为什么 access log 聚合不够），在 code review 里作为明确议题
- **Controller / Center 控制面主动克制新增 metric**：作用域 `src/core/controller/`、`src/core/center/`、`src/core/common/observe/fed_metrics.rs` 等。虽然控制面 cardinality 风险小（触发频率低、label 由部署规模决定），但"容易加"不是"应该加"的理由 —— 控制面事件**默认走 `tracing::{info,warn,error}!` 结构化日志 + K8s status condition（`Accepted/Ready/Programmed`）**，由日志采集侧聚合回答"某类事件发生多少次 / 哪些资源有问题"。只有当事件同时满足以下条件之一，才考虑加 metric：
  - **需要作为秒级告警阈值**（如 `edgion_fed_watch_errors_total` 触发 Prometheus 告警）
  - **Grafana 长期趋势核心 SLI**（低维度、高价值的健康度指标，如 `edgion_fed_connected_controllers`）
  - **跨进程聚合**（单实例日志里看不到全貌，需要 Prometheus 做 sum/rate）
  
  **默认不加 histogram**：控制面目前 0 条 histogram，继续保持。如果要加，需：(a) 说明为什么 `_seconds_last` gauge / `_total` counter 表达不出分布；(b) 说明为什么日志聚合不够；(c) bucket 显式设置避免 `+Inf` 陷阱（参见原 `MIRROR_DURATION_MS` 事故）。
  
  **默认不加 counter/gauge**：validate 拒绝、资源冲突、配置非法等离散事件用 `tracing::warn!(ns, name, field, reason, "...")` + status condition 已经足够；不要"顺手"加 `edgion_controller_validate_rejected_total{reason}` 这类 counter —— 每个 reason 都是一次 cardinality 决策，容易无序扩张。
- **更细的观测走 access log 聚合，不扩指标**：大部分"这一次请求发生了什么/耗时多少/发给哪个 backend"类问题，原始数据已经在 access/mirror/ssl/tls/tcp/udp access log 里（JSON 结构化字段），查询时用 Loki / Elasticsearch / ClickHouse 聚合就能得到任意维度的 percentile / rate / 分布。Prometheus 负责 **秒级告警 + 低基数聚合**；access log 负责 **高基数细节回放**。两者分工清楚：
  - 问 "gateway 现在出错率怎么样？" → Prometheus counter（`edgion_requests_failed_total`）
  - 问 "刚才那个 5xx 请求具体发生了什么？x-trace-id 是 abc123" → access log
  - 问 "mirror 的 p99 耗时？" → mirror access log 的 `et` 字段聚合，**不**加 histogram
  - 问 "某个 backend 的 2xx 比例？" → counter rate，已有
- **严格控制 label 的 cardinality**：label 的值域必须有限且可预测（namespace、name、status group 等）。**绝不使用** path、user_id、trace_id 等高基数值作为 label。此规则数据面 + 控制面都适用
- **混合注册策略**：`GatewayMetrics` 的 handle 按"调用热度"分成两类
  - **Hot（每请求路径）** → `Counter`/`Gauge` 直存，`new()` 中用 `counter!()/gauge!()` 急注册。per-call 零开销；首请求前短暂 0 值可接受（ctx_created、ctx_active、gateway_request_bytes、gateway_response_bytes）
  - **Cold / 失败路径 / 控制面低频** → `OnceLock<Counter>/<Gauge>` 保存，首次 record 调用才注册。未被触发的指标不会出现在 `/metrics`，避免零值噪声与 scrape 浪费
  - 带 label 的 free function（`record_backend_request` / `record_cb_*` / `record_mirror_*` / `record_acme_*`）天然懒注册——label 组合未命中就不存在时序
- **新增指标前先问**：现有指标能否表达？能计算出来的不要额外存？access log 里已有的字段能否用聚合表达（通常能）？
- **命名规范**：`edgion_<component>_<what>_<unit>_total / _active`，全小写下划线

---

## 添加新指标的步骤

`src/core/gateway/observe/metrics/registry.rs` — 唯一的 metrics 定义文件，所有指标统一在此定义。Prometheus 暴露入口在 `src/core/gateway/observe/metrics/api.rs`。

**1. 在 `names` mod 中定义常量名**

```rust
pub mod names {
    pub const MY_NEW_COUNTER: &str = "edgion_gateway_my_event_total";
}
```

**2. 先判定 Hot 还是 Cold，字段类型跟着分**

判定标准：**每请求路径 = Hot**（几秒内必然非 0）；**失败/低频/可能永不触发 = Cold**。

```rust
pub struct GatewayMetrics {
    // --- Hot per-request handles (eager registration) ---
    my_hot_counter: Counter,              // Counter / Gauge only — no Histogram

    // --- Cold / rare-event handles (lazy registration via OnceLock) ---
    my_cold_counter: OnceLock<Counter>,
}
```

**3. 在 `GatewayMetrics::new()` 按类别初始化**

```rust
fn new() -> Self {
    Self {
        // Hot: eager
        my_hot_counter: counter!(names::MY_HOT_COUNTER),
        // Cold: lazy
        my_cold_counter: OnceLock::new(),
    }
}
```

**4. 添加 `#[inline]` record 方法**

```rust
// Hot: 直接 .increment()，零额外开销
#[inline]
pub fn my_hot_event(&self) {
    self.my_hot_counter.increment(1);
}

// Cold: 走 get_or_register_counter / get_or_register_gauge helper
#[inline]
pub fn my_cold_event(&self) {
    get_or_register_counter(&self.my_cold_counter, names::MY_COLD_COUNTER).increment(1);
}
```

> **Gauge 对称性**：Cold gauge 的 decrement 前必须先有对应 increment 触发注册，否则会出现 -1 漂移。参考 `controller_connected_gateways` 的 `gateway_connected` / `gateway_disconnected` 配对。Hot gauge 不受此影响（启动即注册为 0）。

**5. 在调用点使用**

```rust
use crate::core::gateway::observe::metrics::global_metrics;

global_metrics().my_event();
```

---

## 禁止事项

| 禁止 | 作用域 | 原因 |
|------|--------|------|
| `metrics::histogram!(...)` | **Gateway 数据面** | hot-path 上 histogram × label 笛卡尔积容易爆炸。**仅存量 `edgion_gateway_request_duration_ms` 一条例外** |
| label 用 path / user_id / ip 等高基数值 | 数据面 + 控制面 | 导致 metrics 爆炸（时序存储条数 = label 组合数） |
| 每个插件单独注册自己的 metrics | Gateway 数据面 | 分散管理，难以审计总量；统一走 `GatewayMetrics` |
| Counter 用浮点数增量 | 数据面 + 控制面 | 使用整数 `increment(n: u64)` |
| 给 fire-and-forget / shadow traffic 补 latency histogram | 数据面 + 控制面 | 失败信号走 counter（如 `channel_full`、`timeout` status），细节时延查 access log 聚合就够 |

---

## 合理的 Label 使用

Labels 只用于有限枚举值（cardinality ≤ 数十）：

```rust
// ✅ 合理 label：固定枚举
counter!(
    names::BACKEND_REQUESTS_TOTAL,
    "status" => status_group(ctx.request_info.status),   // "2xx"/"3xx"/"4xx"/"5xx"/"failed"
    "protocol" => "grpc",
    "gateway_name" => gateway_name,                       // 实例数量有限
)

// ❌ 危险 label：高基数
counter!(
    "edgion_requests",
    "path" => request_path,    // 路径无限多 → cardinality 爆炸
    "user_id" => user_id,      // 同上
)
```

### 带多维资源 label 的 counter：用配置项 + 条件 push

对于像 `edgion_backend_requests_total`、`edgion_gateway_request_duration_ms` 这类同时带 gateway / route / backend / protocol / status 多维 label 的指标，**不要一把梭 push 所有 label**，而是按 `record_request_duration` / `record_backend_request` 的模式：

1. 在 `gateway/cli/config.rs` 定义 `XxxLabels` struct，字段是每一类资源的 bool 开关。**不要把 `*_namespace` 和 `*_name` 拆成两个开关**——K8s 资源身份天然成对，一个开关控一对
2. 配置挂到 `MetricsConfig`，全局 `OnceLock<...Config>` 在启动时初始化（参考 `init_request_duration_config` / `init_backend_requests_config`）
3. record 函数签名第一个参数传 `&XxxLabels`，内部用 `Vec::with_capacity(N)` + 按 toggle `labels.push(...)` 条件累积，最后 `counter!(..., &labels[..]).increment(1)`
4. Default 值保守：开启 gateway 身份 + 有限枚举 label（protocol/status），不开 route/backend。让运维**显式按需打开** drill-down 维度
5. 运行时 flag 触发的可选 label（test_key / test_data）即使 struct 字段不控，也必须**条件 push（只在非空时 push）**，避免 `label_name=""` 空值占时序

凡新加"每请求路径 × 多维资源 label"counter/gauge，必须走这个模式，否则 cardinality 可能被单个配置失误击穿。

## 可接受的指标类型

| 类型 | Gateway 数据面 | Controller / Center 控制面 | 示例 |
|------|-------------|-------------------------|------|
| `Counter` | ✅ | ✅ | 请求总数、错误总数、字节数 |
| `Gauge` | ✅ | ✅ | 活跃连接数、已连接 gateway 数 |
| `Histogram` | ❌ 禁用（仅 SLI 例外） | ⚠️ 允许但节制，默认 0 条 | 数据面：`edgion_gateway_request_duration_ms`（存量 SLI） |

---

## Gauge 对称性

使用 Gauge 时必须保证增减对称，否则指标长期漂移失去意义。

对 Gauge 操作建议在 `Drop` 实现或明确的 RAII guard 中保证对称性（参考 `ctx_active` Gauge 的 `Drop for EdgionHttpContext` 实现）。

---

## Controller / Center Metrics

Controller 和 Center 是控制面，不跑在数据面 hot-path 上。虽然 cardinality 风险小，但**主动克制新增 metric**（不仅是 histogram，counter/gauge 也一样）—— 控制面事件默认走 `tracing` 结构化日志 + status condition，日志采集侧聚合即可回答大部分"发生了多少次/哪些资源有问题"的问题。

### 控制面加 metric 的决策门槛

新增控制面 metric 前，问自己三个问题，**任一"否"就应该用 tracing/status 替代**：

1. **这个信号要作为秒级告警阈值吗？** 比如 `edgion_fed_watch_errors_total > 0` 触发 oncall —— 是 → 加 counter；否 → `tracing::warn!` + 日志告警
2. **这是 Grafana 长期趋势的核心 SLI 吗？** 比如"联邦连上的 controller 数量" —— 是 → 加 gauge；否 → 查询时直接数 K8s 资源 / `kubectl get` 就能拿到
3. **单实例日志里看不到全貌吗？** 比如需要聚合所有 controller 的某类事件总数 —— 是 → 加 metric；否 → 用结构化日志字段查询

**不应该加 metric 的典型信号**（用 tracing + status condition 替代）：
- validate 拒绝类事件（非法 match_type、未知 allowedRoutes.from、schema 字段错配等）→ `tracing::warn!(ns, name, field, reason)` + Accepted:False condition，日志侧 `{reason="UnsupportedMatchType"}` 聚合就够
- 资源 parse / reconcile 失败的具体原因分类 → 已经在 status condition 里了，不要再用 counter 复述
- "某种配置组合被用了多少次" → 日志 debug/info 记录，查询时聚合

实施规则：

1. **统一入口**：全局 `PrometheusRecorder` 安装走 `src/core/common/observe/metrics_api.rs::install_global_recorder(RecorderConfig)`。两个二进制在 `run()` 入口附近各调一次（参数 `service: "edgion-controller"` / `"edgion-center"`）
2. **`/metrics` 复用 admin router**：调用 `crate::core::common::observe::metrics_api::metrics_handler` 挂到各自 axum Router 的 `/metrics` path，不另开端口
3. **Auth skip**：`/metrics` 已在 `src/core/common/unified_auth/mod.rs` 的 `skip_paths` 里写死，与 `/health`、`/ready` 一起免 auth。新增敏感路由不要改这个 skip 列表
4. **注册风格**：沿用 `src/core/controller/conf_mgr/sync_runtime/metrics.rs` 的风格——直接 `counter!/gauge!` 宏，不用 `OnceLock` lazy 封装（controlplane 调用频次低，急注册即可）
5. **Federation 指标**：全部走 `src/core/common/observe/fed_metrics.rs`（`edgion_fed_*` 前缀），Center/Controller 共用。Label 值域**必须**是 `labels::` 子模块里的 `&'static str` 常量，禁止直接拼字符串
6. **如果真要加 histogram**：控制面允许，但需：(a) 说明为什么 `_seconds_last` gauge / counter 表达不出；(b) 说明为什么 access log 聚合不够；(c) bucket 要显式设 `set_buckets_for_metric` 或通过 `RecorderConfig.histogram_buckets` 注入（避免默认秒 bucket 与 ms 记录值错配导致数据全落 `+Inf`，参见原 `MIRROR_DURATION_MS` 事故）

## Test Metrics 例外

`src/core/gateway/observe/metrics/test_metrics.rs` 是专为集成测试设计的测试专用数据收集模块，**不受上述生产 metrics 规则约束**。

### 特点

- 只在 `--integration-testing-mode` 开启时激活
- 通过 Gateway annotation（`edgion.io/metrics-test-type`）显式开启，生产环境不会触发
- `TestType` 枚举化控制收集的数据类型（`Lb` / `Retry` / `Latency`）
- 数据以 `test_data` JSON label 附加在 `backend_requests_total` 中，供测试断言

### 新增测试数据类型

在 `metrics/test_metrics.rs` 中：

1. 在 `TestType` 新增枚举值
2. 在 `TestData` 新增对应字段（`#[serde(skip_serializing_if = "Option::is_none")]`）
3. 新增 `set_xxx_test_data()` 函数
4. 在 `pg_logging.rs` 的 `build_test_data()` 中的 `match test_type` 分支处理

### 场景：需要监控某类事件的频率

先检查 `metrics/registry.rs` 中是否已有合适的 Counter。如果没有：
1. 确认 cardinality 可控
2. 按上方步骤在 `metrics/registry.rs` 统一添加
3. 不要在各自模块里用 `metrics::counter!()` 宏直接创建游离指标

### 场景：集成测试需要验证某个细节行为

在 `metrics/test_metrics.rs` 中扩展 `TestData`，不要污染生产 access log 或 metrics。测试数据通过 Gateway annotation 显式激活，生产环境零开销。
