# Metrics 规范

> Metrics 核心原则：避免 metrics 爆炸。无 Histogram、严控 Label 基数、统一在 `GatewayMetrics` 管理。

## 核心原则

- **不引入 Histogram 类型**（无 `_bucket` / `_sum` / `_count` 三元组）
- **严格控制 label 的 cardinality**：label 的值域必须有限且可预测（namespace、name、status group 等）。**绝不使用** path、user_id、trace_id 等高基数值作为 label
- **混合注册策略**：`GatewayMetrics` 的 handle 按"调用热度"分成两类
  - **Hot（每请求路径）** → `Counter`/`Gauge` 直存，`new()` 中用 `counter!()/gauge!()` 急注册。per-call 零开销；首请求前短暂 0 值可接受（ctx_created、ctx_active、gateway_request_bytes、gateway_response_bytes）
  - **Cold / 失败路径 / 控制面低频** → `OnceLock<Counter>/<Gauge>` 保存，首次 record 调用才注册。未被触发的指标不会出现在 `/metrics`，避免零值噪声与 scrape 浪费
  - 带 label 的 free function（`record_backend_request` / `record_cb_*` / `record_mirror_*` / `record_acme_*`）天然懒注册——label 组合未命中就不存在时序
- **新增指标前先问**：现有指标能否表达？能计算出来的不要额外存
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

| 禁止 | 原因 |
|------|------|
| `metrics::histogram!(...)` | 不引入 Histogram |
| label 用 path / user_id / ip 等高基数值 | 导致 metrics 爆炸（时序存储条数 = label 组合数） |
| 每个插件单独注册自己的 metrics | 分散管理，难以审计总量 |
| Counter 用浮点数增量 | 使用整数 `increment(n: u64)` |

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

## 可接受的指标类型

| 类型 | 用途 | 示例 |
|------|------|------|
| `Counter` | 只增不减的累计量 | 请求总数、错误总数、字节数 |
| `Gauge` | 当前瞬时值（可增可减） | 活跃连接数、已连接 gateway 数 |
| ~~`Histogram`~~ | ~~分布统计~~ | **不引入** |

---

## Gauge 对称性

使用 Gauge 时必须保证增减对称，否则指标长期漂移失去意义。

对 Gauge 操作建议在 `Drop` 实现或明确的 RAII guard 中保证对称性（参考 `ctx_active` Gauge 的 `Drop for EdgionHttpContext` 实现）。

---

## Controller / Center Metrics

Controller 和 Center 不像 Gateway 那样跑在热路径上，并且都只使用 Counter / Gauge。实施规则：

1. **统一入口**：全局 `PrometheusRecorder` 安装走 `src/core/common/observe/metrics_api.rs::install_global_recorder(RecorderConfig)`。两个二进制在 `run()` 入口附近各调一次（参数 `service: "edgion-controller"` / `"edgion-center"`）
2. **`/metrics` 复用 admin router**：调用 `crate::core::common::observe::metrics_api::metrics_handler` 挂到各自 axum Router 的 `/metrics` path，不另开端口
3. **Auth skip**：`/metrics` 已在 `src/core/common/unified_auth/mod.rs` 的 `skip_paths` 里写死，与 `/health`、`/ready` 一起免 auth。新增敏感路由不要改这个 skip 列表
4. **注册风格**：沿用 `src/core/controller/conf_mgr/sync_runtime/metrics.rs` 的风格——直接 `counter!/gauge!` 宏，不用 `OnceLock` lazy 封装（controlplane 调用频次低，急注册即可）
5. **Federation 指标**：全部走 `src/core/common/observe/fed_metrics.rs`（`edgion_fed_*` 前缀），Center/Controller 共用。Label 值域**必须**是 `labels::` 子模块里的 `&'static str` 常量，禁止直接拼字符串

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
