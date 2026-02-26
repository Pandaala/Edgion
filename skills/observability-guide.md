# Edgion Observability Guide

> Quick reference for adding access log fields and metrics in Edgion.
> Follow these principles to keep observability lean, powerful, and maintainable.

---

## Core Principles

### Access Log: 单条日志能还原整个请求

每一条 access log 应当包含足够的信息，让运维人员无需翻其他日志就能定位问题。关注点包括：

- **路由命中情况**（哪条 route / backend 被选中）
- **插件执行关键结果**（认证通过/拒绝、限流触发、改写了什么）
- **上游连接情况**（重试次数、latency 各阶段、最终状态码）
- **请求身份信息**（real IP、trace-id、sni）

### Access Log: 只记录关键节点，不记过程

❌ **不要记录**：
- 插件内部的中间状态（"starting auth check"、"token parsed"）
- 每次 header 读取、每次条件判断
- 大量重复的 debug 信息（这些应进 `tracing::debug!()`）

✅ **要记录**：
- 插件的**最终决策**（允许 / 拒绝 / 改写了什么值）
- **异常路径**（连接失败、超时、校验错误的原因摘要）
- **关键参数**（命中的 key、选中的 backend、retry 次数）

### Metrics: 避免 metrics 爆炸

- **不引入 Histogram 类型**（无 `_bucket` / `_sum` / `_count` 三元组）
- **严格控制 label 的 cardinality**：label 的值域必须有限且可预测（namespace、name、status group 等）。**绝不使用** path、user_id、trace_id 等高基数值作为 label
- **新增指标前先问**：现有指标能否表达？能计算出来的不要额外存
- **命名规范**：`edgion_<component>_<what>_<unit>_total / _active`，全小写下划线

---

## Test Metrics 例外说明

**`src/core/observe/test_metrics.rs`** 是专为集成测试设计的测试专用数据收集模块，**不受上述生产 metrics 规则约束**。

### 特点

- 只在 `--integration-testing-mode` 开启时激活（`is_integration_testing_mode()` 检测）
- 通过 Gateway annotation（`edgion.io/metrics-test-type`）显式开启，生产环境不会触发
- `TestType` 枚举化控制收集的数据类型（`Lb` / `Retry` / `Latency`）
- 数据以 `test_data` JSON label 附加在 `backend_requests_total` 中，供测试断言

### 可以做的（测试 metrics 专属）

- 高基数 label（ip、port、hash_key、error 消息等）— 测试场景数据量可控
- 详细的中间状态（latency_ms、try_count 等）— 用于验证算法正确性
- 任意自定义字段 — 按验证需求设计 `TestData` 结构

### 新增测试数据类型

在 `test_metrics.rs` 中：

1. 在 `TestType` 新增枚举值
2. 在 `TestData` 新增对应字段（`#[serde(skip_serializing_if = "Option::is_none")]`）
3. 新增 `set_xxx_test_data()` 函数
4. 在 `pg_logging.rs` 的 `build_test_data()` 中的 `match test_type` 分支处理

---

## Access Log 字段设计

### 现有 AccessLogEntry 结构（`src/core/observe/access_log/entry.rs`）

access log 由 `AccessLogEntry::from_context(ctx)` 在 `pg_logging.rs` 的 `logging()` 阶段生成，包含：

| 字段 | 含义 |
|------|------|
| `client-addr` | TCP 直连 IP |
| `remote-addr` | 真实客户端 IP（经 RealIp 插件处理后） |
| `host` / `path` | 请求 host 和 path |
| `x-trace-id` | 客户端传入的 trace id |
| `status` | 最终响应状态码 |
| `sni` | TLS SNI（HTTPS 连接） |
| `tls_id` | TLS 连接 ID（用于关联 tls.log） |
| `discover_protocol` | 自动探测协议（grpc / websocket 等） |
| stage_logs | 各阶段插件日志（见下方 PluginLog） |
| upstreams | 上游连接详情（ip、port、ct/ht/bt/et、retry） |

### 何时向 AccessLogEntry 增加新字段

**满足以下条件才增加：**

1. 该字段在排障时**高频需要**，且无法从其他现有字段推断
2. 字段是**请求级别**的（不是插件内部的中间状态）
3. 字段值**不会导致存储爆炸**（避免把大 body 或长 token 放进去）

**正确示例**：
- 新增 `grpc_service` / `grpc_method`（gRPC 路由排障必要信息）✅
- 新增 `client_cert_info`（mTLS 开启后需要知道是哪个证书） ✅

**错误示例**：
- 把整个请求 body 放进 access log ❌
- 把每个 header 都展开进 access log ❌

---

## Plugin Log (PluginLog) 规范

每个插件通过 `log: &mut PluginLog` 向 access log 写入紧凑的执行摘要。

### 格式规范

```
<结果> [<关键参数>]; 
```

- 以 `; ` 结尾（方便 grep / 拼接）
- 结果词首选大写缩写：`OK`、`Deny`、`Skip`、`Fail`、`Rate`
- 关键参数用 `k=v` 格式，多个参数空格分隔

### 好的 PluginLog 示例

```rust
// KeyAuth 插件 - 认证通过
log.push("OK u=jack; ");

// KeyAuth 插件 - key 不存在
log.push("Deny no-key; ");

// RateLimit 插件 - 触发限流
log.push(&format!("Rate 429 k=user:{}; ", user_id));

// RateLimit 插件 - 未触发
log.push("OK; ");

// BasicAuth 插件 - 认证失败（带原因摘要）
log.push("Deny bad-cred; ");

// Redirect 插件 - 执行了跳转
log.push(&format!("-> {} {}; ", code, location));

// DirectEndpoint 插件 - 强制指定了 endpoint
log.push(&format!("Direct {}:{}; ", ip, port));

// Retry - 记录重试信息（pg_logging 阶段）
// 通过 ctx.try_cnt 获取，放进 UpstreamInfo 而不是 PluginLog
```

### 不好的 PluginLog 示例

```rust
// ❌ 太啰嗦，占满 buffer
log.push("Starting JWT authentication process for incoming request");
log.push("Successfully parsed and validated JWT token with RSA256 algorithm");

// ❌ 内部细节，不是最终决策
log.push("Checking rate limit window...");
log.push("Window size = 60s");

// ❌ 应该用 tracing::debug! 的内容
log.push(&format!("token_claims = {:?}", claims));
```

### PluginLog buffer 限制

`PluginLog` 有固定大小的 buffer（默认 100 字节）。每条日志应保持在 **20~40 字节**以内，留出多个插件同时写入的空间。超出 buffer 的内容会被截断。

详细调试信息请用：

```rust
tracing::debug!(
    component = "my_plugin",
    user = %user_id,
    "JWT validated"
);
```

---

## Metrics 添加规范

### 现有 Metrics 文件

`src/core/observe/metrics.rs` — 唯一的 metrics 定义文件，所有指标统一在此定义。

### 添加新指标的步骤

**1. 在 `names` mod 中定义常量名**

```rust
pub mod names {
    // 命名规范: edgion_<component>_<what>_<unit_or_qualifier>
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

### 禁止事项

| 禁止 | 原因 |
|------|------|
| `metrics::histogram!(...)` | 不引入 Histogram |
| label 用 path / user_id / ip 等高基数值 | 导致 metrics 爆炸（时序存储条数 = label 组合数） |
| 每个插件单独注册自己的 metrics | 分散管理，难以审计总量 |
| Counter 用浮点数增量 | 使用整数 `increment(n: u64)` |

### 合理的 Label 使用

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

### 可接受的指标类型

| 类型 | 用途 | 示例 |
|------|------|------|
| `Counter` | 只增不减的累计量 | 请求总数、错误总数、字节数 |
| `Gauge` | 当前瞬时值（可增可减） | 活跃连接数、已连接 gateway 数 |
| ~~`Histogram`~~ | ~~分布统计~~ | **不引入** |

---

## 观测最佳实践

### 1. 信息分层：三层日志各司其职

不同观测手段有不同定位，不要混用：

| 层次 | 工具 | 目标受众 | 保留时间 |
|------|------|---------|---------|
| 请求粒度 | access log（JSON） | 运维排障、审计 | 天~周 |
| 系统事件 | `tracing::info/warn/error` | 开发调试、告警 | 小时~天 |
| 性能聚合 | Prometheus metrics | SRE、Grafana | 月 |

**原则**：access log 聚焦请求维度；系统日志聚焦组件事件；metrics 聚焦趋势聚合。三者互补，不要用 access log 当系统日志用，也不要用 metrics 存原始请求细节。

### 2. Tracing 结构化日志规范

使用 key-value 结构化字段，便于日志系统索引：

```rust
// ✅ 正确：结构化字段
tracing::warn!(
    component = "grpc_client",
    error = %e,
    backoff_secs = backoff.as_secs(),
    "WatchServerMeta connection failed, retrying"
);

// ❌ 错误：拼接字符串，丢失结构
tracing::warn!("WatchServerMeta connection failed: {}, retry in {}s", e, backoff.as_secs());
```

**固定使用 `component` 字段标识模块**，方便按组件过滤日志：

```rust
// 固定 component 名，小写下划线，对应文件/模块名
tracing::info!(component = "grpc_server", ...);
tracing::info!(component = "k8s_status", ...);
tracing::info!(component = "conf_client", ...);
```

**选择正确的 level**：

| Level | 适用场景 |
|-------|---------|
| `error!` | 影响请求/数据的错误（需要立即处理） |
| `warn!` | 可恢复的异常、重试、降级（需要关注但不紧急） |
| `info!` | 重要的生命周期事件（启动、关闭、reload、连接建立/断开） |
| `debug!` | 排查时有用的中间状态（正常运行时不需要看） |
| `trace!` | 极度详细的流程追踪（性能分析专用） |

### 3. 不要在热路径上做高开销 observability 操作

请求处理的热路径（`request_filter`、`upstream_peer`、`response_filter` 等）每个请求都会执行，要避免：

```rust
// ❌ 热路径上的高开销操作
tracing::info!("Processing request {}", req_id);  // info! 在高并发下有性能开销
serde_json::to_string(&full_ctx)?;                 // 序列化整个 ctx

// ✅ 热路径用 inline + 低开销
global_metrics().request_success();   // #[inline] Counter increment，纳秒级
tracing::debug!(...);                  // debug! 在 release 构建中编译期消除
```

**logging 阶段（每请求只执行一次）** 是唯一适合做稍重操作的地方：序列化 access log、发送 channel 等。

### 4. Gauge 的对称性：每个 +1 必须有对应的 -1

使用 Gauge 时必须保证增减对称，否则指标长期漂移失去意义：

```rust
// ✅ 正确：在所有退出路径都 -1
registry.register(...);
global_metrics().gateway_connected();   // +1

// 退出路径 1: 正常断开
registry.unregister(&client_id);
global_metrics().gateway_disconnected(); // -1

// 退出路径 2: 初始发送失败
registry.unregister(&client_id);
global_metrics().gateway_disconnected(); // -1（不要漏掉！）

// 退出路径 3: server reload
registry.unregister(&client_id);
global_metrics().gateway_disconnected(); // -1
```

对 Gauge 操作建议在 `Drop` 实现或明确的 RAII guard 中保证对称性（参考 `ctx_active` Gauge 的 `Drop for EdgionHttpContext` 实现）。

### 5. 敏感信息不进日志

以下内容**绝对不能**出现在 access log 或 tracing 日志中：

| 敏感信息 | 处理方式 |
|---------|---------|
| API Key / Token 原文 | 记录前 N 字符 + `***`，或只记录 hash |
| 密码 / 证书私钥 | 完全不记录，只记录操作结果 |
| 用户个人信息（姓名、手机等） | 不记录，或脱敏后记录 |
| 完整请求 body | 只记录 body size，不记录内容 |

```rust
// ✅ 安全：只记录 key 前缀
let key_prefix = &api_key[..api_key.len().min(8)];
log.push(&format!("OK k={}***; ", key_prefix));

// ❌ 危险：记录原始 token
log.push(&format!("OK token={}; ", full_token));
```

### 6. 错误信息要有定位价值

错误日志要包含足够的上下文，让人能快速定位根因：

```rust
// ✅ 有价值：包含 what/where/why
tracing::error!(
    component = "k8s_status",
    kind = "HTTPRoute",
    namespace = namespace,
    name = name,
    error = %e,
    "Failed to update K8s status"
);

// ❌ 无价值：看不出发生了什么
tracing::error!("error: {}", e);
```

### 7. 慎用 `unwrap` / `expect`，优先记录 warn 并降级

在 observability 相关代码中，数据收集失败不应该导致请求失败：

```rust
// ✅ 优雅降级：收集失败不影响请求
let json = serde_json::to_string(&entry).unwrap_or_else(|e| {
    tracing::warn!(component = "access_log", error = %e, "Failed to serialize log entry");
    String::new()
});

// ❌ panic 导致请求中断
let json = serde_json::to_string(&entry).unwrap();
```

### 8. 给 tracing span 的 instrument 合理命名

涉及异步任务的长运行逻辑，善用 `instrument` 给 span 命名：

```rust
use tracing::instrument;

#[instrument(name = "watch_server_meta", skip(self), fields(client_id = %self.client_id))]
pub async fn start_watch_server_meta(self: Arc<Self>) {
    // ...
}
```

这样在分布式追踪系统（如 Jaeger）中可以按 span 名称过滤和关联。

---

## 常见场景速查

### 场景 1：插件做了认证决策

```rust
// 在 plugin.rs 的 run_request 中
log.push("OK u=alice; ");          // 通过
log.push("Deny missing-token; ");  // 拒绝，附原因
```

不需要加 metrics（认证结果已体现在 `requests_failed` 和 `backend_requests_total` 的 status 中）。

### 场景 2：插件改写了请求

```rust
// 记录改写动作和结果，不记录原始值（可能含敏感信息）
log.push(&format!("Rewrite -> {}; ", new_path));
```

### 场景 3：需要监控某类事件的频率

先检查 `metrics.rs` 中是否已有合适的 Counter。如果没有：

1. 确认 cardinality 可控
2. 按上方步骤在 `metrics.rs` 统一添加
3. 不要在各自模块里用 `metrics::counter!()` 宏直接创建游离指标

### 场景 4：上游连接失败

上游的 error 信息通过 `UpstreamInfo.err` 字段进入 access log，不需要在插件里单独记录。`requests_failed` counter 会在 `pg_logging.rs` 统一更新。

### 场景 5：需要 tracing 日志还是 access log

| 信息类型 | 去处 |
|---------|------|
| 请求最终决策 | `PluginLog` → access log |
| 排查中间步骤 | `tracing::debug!()` → 系统日志 |
| 异常/告警 | `tracing::warn!()` / `tracing::error!()` → 系统日志 |
| 性能数据 | `Gauge` / `Counter` in `metrics.rs` → Prometheus |

### 场景 6：集成测试需要验证某个细节行为

在 `test_metrics.rs` 中扩展 `TestData`，不要污染生产 access log 或 metrics。测试数据通过 Gateway annotation 显式激活，生产环境零开销。

---

## Checklist：提交前自检

- [ ] PluginLog 每条 ≤ 40 字节，以 `; ` 结尾
- [ ] 没有把调试信息写进 PluginLog（应用 tracing::debug）
- [ ] 新增 metrics 通过 `GatewayMetrics` struct 管理，不直接用 `metrics::counter!()` 宏
- [ ] 新增 metrics 无 Histogram 类型
- [ ] Label 值域有限（≤ 数十个），无高基数 label
- [ ] Gauge 的所有 +1 路径都有对应的 -1（检查所有退出路径）
- [ ] 新增字段在 access log 中确实有排障价值，不是"以防万用"
- [ ] 没有把敏感信息（token、密码、完整 body）写进任何日志
- [ ] tracing 日志有 `component` 字段，error/warn 日志有足够的定位上下文
- [ ] 热路径上的 observability 操作是低开销的（no info!/no serde in request_filter）