# Metrics 规范

> Metrics 核心原则：避免 metrics 爆炸。无 Histogram、严控 Label 基数、统一在 `GatewayMetrics` 管理。

## 核心原则

- **不引入 Histogram 类型**（无 `_bucket` / `_sum` / `_count` 三元组）
- **严格控制 label 的 cardinality**：label 的值域必须有限且可预测（namespace、name、status group 等）。**绝不使用** path、user_id、trace_id 等高基数值作为 label
- **新增指标前先问**：现有指标能否表达？能计算出来的不要额外存
- **命名规范**：`edgion_<component>_<what>_<unit>_total / _active`，全小写下划线

---

## 添加新指标的步骤

`src/core/observe/metrics.rs` — 唯一的 metrics 定义文件，所有指标统一在此定义。

**1. 在 `names` mod 中定义常量名**

```rust
pub mod names {
    pub const MY_NEW_COUNTER: &str = "edgion_gateway_my_event_total";
}
```

**2. 在 `GatewayMetrics` struct 中添加字段**

```rust
pub struct GatewayMetrics {
    // ...
    /// Brief description of what this tracks
    my_new_counter: Counter,   // Counter / Gauge only — no Histogram
}
```

**3. 在 `GatewayMetrics::new()` 中初始化**

```rust
fn new() -> Self {
    Self {
        // ...
        my_new_counter: counter!(names::MY_NEW_COUNTER),
    }
}
```

**4. 添加 `#[inline]` 方法**

```rust
/// Record a foo event
#[inline]
pub fn my_event(&self) {
    self.my_new_counter.increment(1);
}
```

**5. 在调用点使用**

```rust
use crate::core::observe::metrics::global_metrics;

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

## Test Metrics 例外

`src/core/observe/test_metrics.rs` 是专为集成测试设计的测试专用数据收集模块，**不受上述生产 metrics 规则约束**。

### 特点

- 只在 `--integration-testing-mode` 开启时激活
- 通过 Gateway annotation（`edgion.io/metrics-test-type`）显式开启，生产环境不会触发
- `TestType` 枚举化控制收集的数据类型（`Lb` / `Retry` / `Latency`）
- 数据以 `test_data` JSON label 附加在 `backend_requests_total` 中，供测试断言

### 新增测试数据类型

在 `test_metrics.rs` 中：

1. 在 `TestType` 新增枚举值
2. 在 `TestData` 新增对应字段（`#[serde(skip_serializing_if = "Option::is_none")]`）
3. 新增 `set_xxx_test_data()` 函数
4. 在 `pg_logging.rs` 的 `build_test_data()` 中的 `match test_type` 分支处理

### 场景：需要监控某类事件的频率

先检查 `metrics.rs` 中是否已有合适的 Counter。如果没有：
1. 确认 cardinality 可控
2. 按上方步骤在 `metrics.rs` 统一添加
3. 不要在各自模块里用 `metrics::counter!()` 宏直接创建游离指标

### 场景：集成测试需要验证某个细节行为

在 `test_metrics.rs` 中扩展 `TestData`，不要污染生产 access log 或 metrics。测试数据通过 Gateway annotation 显式激活，生产环境零开销。
