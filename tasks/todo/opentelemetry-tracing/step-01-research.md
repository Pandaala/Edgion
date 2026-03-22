# Step-01 研究报告：为 Edgion Gateway 接入 OpenTelemetry 分布式追踪

> 日期：2026-03-22
> 分支：feature-03-16

---

## 1. 当前状态

### 1.1 现有 tracing 体系

Edgion 目前的可观测性分为三层：

| 层次 | 工具 | 用途 |
|------|------|------|
| 请求粒度 | Access Log（JSON，写文件） | 运维排障、审计 |
| 系统事件 | `tracing` crate（结构化日志） | 控制面组件事件 |
| 性能聚合 | Prometheus metrics（`metrics` crate） | SRE、Grafana |

**数据面**（请求热路径）明确禁止使用 `tracing::*` 做请求级日志（见 `01-log-safety.md`）。请求信息通过 access log 承载，写入 `EdgionHttpContext`，在 `logging` hook 统一输出。

### 1.2 trace_id / request_id 机制

当前 ID 体系（见 `src/types/ctx.rs` `RequestInfo`）：

```
x_trace_id: Option<String>
```

生成逻辑在 `pg_request_filter.rs` 的 `build_request_metadata()`：

1. 读取入站请求头 `x-trace-id`
2. 若不存在，则 `uuid::Uuid::new_v4().to_string()` 生成新 UUID
3. 存入 `ctx.request_info.x_trace_id`
4. 随 access log 输出（`request_info` 字段）

**关键缺失**：
- 没有读取 W3C `traceparent` 头（`00-{trace_id}-{span_id}-{flags}`）
- 没有向上游注入 `traceparent` 头（无法传播追踪上下文）
- 没有创建 OTel Span，无法向 Jaeger/Tempo/Zipkin 上报

### 1.3 当前 tracing 相关依赖（Cargo.toml）

```toml
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
tracing-appender = "0.2"
```

**无** `opentelemetry`、`opentelemetry-otlp`、`tracing-opentelemetry` 依赖。

### 1.4 Pingora 框架对 OTel 的原生支持

Pingora 版本：`0.8.0`。Pingora 本身**没有内置 OpenTelemetry 支持**。它提供的 `ProxyHttp` trait 各 hook 是纯 Rust 异步函数，追踪逻辑需要完全由应用层自己实现。Pingora 没有 middleware 机制，每个 hook 都需要手动插入 span 操作。

---

## 2. 技术方案选型

### 2.1 方案对比

| 方案 | 描述 | 优点 | 缺点 |
|------|------|------|------|
| **A：tracing-opentelemetry bridge** | 用 `tracing` macro 创建 span，通过 `tracing-opentelemetry` layer 将 span 导出到 OTel | 与现有 `tracing` 代码完全兼容；控制面日志和追踪统一 | 数据面禁止 tracing，bridge 无法在热路径使用；span 粒度受限于 tracing instrument |
| **B：直接使用 OTel SDK（推荐）** | 直接使用 `opentelemetry` SDK 创建/管理 span，与现有 tracing 并列 | 数据面完全控制 span 生命周期；不受 `tracing` 热路径约束；span 字段完全可控 | 需要手动在每个 hook 传递 span context；代码量稍多 |

**推荐方案 B**：直接使用 OTel SDK。

理由：
1. 数据面明确禁止 `tracing::*` 在请求热路径，方案 A 的 bridge 与此冲突
2. 数据面 span 的生命周期完全对应 Pingora hook（request_filter → logging），需要精确控制，OTel SDK 更直接
3. 控制面仍可以用 `tracing-opentelemetry` bridge（可选，future work）
4. 生产级 API Gateway 通常都直接操作 OTel SDK

### 2.2 推荐 Cargo 依赖

```toml
# OpenTelemetry 核心
opentelemetry = { version = "0.27", features = ["trace"] }
opentelemetry_sdk = { version = "0.27", features = ["rt-tokio", "trace"] }

# OTLP 导出器（支持 gRPC 和 HTTP，可按需选择）
opentelemetry-otlp = { version = "0.27", features = [
    "grpc-tonic",   # gRPC/OTLP（推荐，标准协议）
    "http-proto",   # HTTP/OTLP（备选）
    "trace",
] }

# W3C TraceContext 传播（traceparent/tracestate）
opentelemetry-http = { version = "0.27" }

# 可选：让控制面 tracing span 也能导出到 OTel（future work）
# tracing-opentelemetry = "0.28"
```

版本说明：OTel Rust SDK 的最新稳定版为 0.27.x（截至 2026-03 的主流版本）。需与 `tonic` 版本兼容（项目现用 `tonic = "0.12"`，`opentelemetry-otlp` 0.27 的 `grpc-tonic` feature 依赖 tonic 0.12，兼容）。

---

## 3. 实现路径

### 3.1 总体架构

```
入站请求
    ↓
[request_filter] 读取/生成 traceparent，创建 root span，存入 ctx
    ↓
[upstream_peer / upstream_request_filter] 向上游注入 traceparent 头，创建 child span
    ↓
[upstream_response_filter] 记录上游响应状态到 span
    ↓
[logging] 结束 span，设置最终状态码、错误信息，flush
    ↓
OTLP Exporter → Jaeger / Tempo / Zipkin
```

### 3.2 分步实现

#### Step A：OTel 初始化（启动流程）

**文件**：`src/core/gateway/cli/mod.rs`（`EdgionGatewayCli::run()`）

在第 4 步（init_logging）之后，增加 OTel tracer 初始化：

```
run() 流程：
  1. load config
  2. init work_dir
  3. init_logging
→ 4. init_otel_tracer(config.otel)   ← 新增
  5. create pingora server
  6. run_server
```

初始化函数位置：`src/core/gateway/observe/tracing/mod.rs`（新建子模块）

初始化内容：
- 读取 OTLP endpoint（`config.otel.otlp_endpoint`）
- 创建 `opentelemetry_otlp::SpanExporter`
- 创建 `opentelemetry_sdk::trace::TracerProvider`（含批量导出 + 采样率）
- 设置全局 `TracerProvider`（`opentelemetry::global::set_tracer_provider`）
- 返回 `TracerProvider` guard（需在 `run()` 返回前 shutdown）

#### Step B：Span Context 存储

**文件**：`src/types/ctx.rs`

在 `EdgionHttpContext` 中新增字段：

```rust
/// OTel span context for distributed tracing
/// - None: OTel not configured or span not created
pub otel_span: Option<opentelemetry::trace::Span>,
/// Extracted/generated trace context (for upstream propagation)
pub trace_context: Option<opentelemetry::trace::SpanContext>,
```

注：`Span` 不是 Clone，需要用 `Option<Span>` 并在 logging 阶段 take() 结束。

#### Step C：入站 traceparent 处理（request_filter hook）

**文件**：`src/core/gateway/routes/http/proxy_http/pg_request_filter.rs`

在 `build_request_metadata()` 末尾（x_trace_id 生成之后）：

```
1. 读取入站 `traceparent` 头（W3C 格式：00-{trace_id}-{parent_span_id}-{flags}）
2. 用 opentelemetry-http 的 TextMapPropagator 提取 SpanContext
3. 若提取成功：以该 SpanContext 为 parent 创建新 span（"gateway.request"）
4. 若不存在：创建新 root span（生成新 trace_id）
5. 将 span 存入 ctx.otel_span
6. 用 OTel trace_id 覆盖 ctx.request_info.x_trace_id（保证 access log 与 trace 的 ID 一致）
```

Span 属性（attributes）：

```
http.method, http.url, http.host, http.scheme
net.peer.ip (client_addr)
http.user_agent
gateway.listener_port
```

#### Step D：向上游注入 traceparent（upstream_request_filter hook）

目前 proxy_http 目录**没有** `upstream_request_filter` 的独立文件（Pingora 的 `ProxyHttp::upstream_request_filter` 默认是空实现）。需新增：

**文件**：`src/core/gateway/routes/http/proxy_http/pg_upstream_request_filter.rs`（新建）

在 `mod.rs` 中注册到 `ProxyHttp` trait impl。

逻辑：
```
1. 从 ctx.otel_span 提取当前 SpanContext
2. 创建 child span（"gateway.upstream"），设置 upstream ip/port 属性
3. 用 TextMapPropagator::inject() 将 traceparent 写入上游请求头
4. 将 child span 也存入 ctx（或作为 otel_span 的子 span 管理）
```

#### Step E：记录上游响应（upstream_response_filter hook）

**文件**：`src/core/gateway/routes/http/proxy_http/pg_upstream_response_filter.rs`（已有）

在现有逻辑末尾：
```
1. 从 ctx.otel_span 获取当前 span
2. 设置 http.status_code 属性
3. 若 status >= 500，设置 span status 为 Error
```

#### Step F：结束 Span（logging hook）

**文件**：`src/core/gateway/routes/http/proxy_http/pg_logging.rs`（已有）

在 `logging()` 函数末尾（access log 输出之后）：
```
1. take ctx.otel_span（消费 span）
2. 设置最终 http.status_code（覆盖，来自 session.response_written()）
3. 记录 edgion_status 错误码（若有，作为 span event）
4. span.end()（触发批量导出）
```

---

## 4. Pingora 集成要点

Pingora `ProxyHttp` trait 关键 hook 与 OTel 操作的对应关系：

| Hook | OTel 操作 | 说明 |
|------|-----------|------|
| `request_filter` | 创建 root span，读取入站 traceparent | 请求最早入口，此处建立追踪上下文 |
| `upstream_request_filter`（新增） | 注入 traceparent 到上游请求头 | 上游请求发出前，实现 context propagation |
| `connected_to_upstream` | 记录 connect time 到 span | 可选，已有 UpstreamInfo.ct 可复用 |
| `upstream_response_filter` | 记录上游响应状态 | 已有文件，追加 OTel 操作 |
| `logging` | 结束 span，flush | 请求生命周期结束，必须在此 end() |

注意事项：
1. Pingora 的 retry 机制会在 `upstream_peer` 中多次调用，每次 retry 应创建新的 child span（不是替换 root span）
2. `ctx` 在 retry 间通过 `Clone` 传递（Pingora 的设计），`Span` 不可 Clone，需要用 `Arc<Mutex<Span>>` 或重新创建
3. `logging` hook 是请求的终点，无论成功/失败都会调用，是结束 span 的唯一可靠位置

**Retry 的 Span 策略**：root span（gateway.request）跨越整个请求生命周期，每次 retry 创建独立的 child span（gateway.upstream.attempt.{n}），child span 在各自的 `logging` / `fail_to_connect` 结束。

---

## 5. 配置设计

### 5.1 新增配置项结构

在 `EdgionGatewayConfig`（`src/core/gateway/cli/config/`）中新增 `otel` 块：

```yaml
# edgion-gateway.yaml
otel:
  enabled: true                                    # 是否启用 OTel（默认 false）
  otlp_endpoint: "http://tempo:4317"               # OTLP gRPC endpoint（必填）
  # otlp_protocol: "grpc"                          # 协议：grpc（默认）或 http
  service_name: "edgion-gateway"                   # OTel resource.service.name
  service_namespace: "production"                  # OTel resource.service.namespace（可选）
  sampling_ratio: 1.0                              # 采样率（0.0-1.0，默认 1.0 全采样）
  # b3_propagation: false                          # 是否同时支持 B3 格式（Zipkin，默认 false）
  # max_queue_size: 2048                           # 批量导出队列大小
  # export_timeout_secs: 10                        # 导出超时
```

对应 Rust 结构体：

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OtelConfig {
    pub enabled: bool,
    pub otlp_endpoint: Option<String>,
    pub otlp_protocol: Option<String>,      // "grpc" | "http"
    pub service_name: Option<String>,
    pub service_namespace: Option<String>,
    pub sampling_ratio: Option<f64>,
    pub b3_propagation: Option<bool>,
    pub max_queue_size: Option<usize>,
    pub export_timeout_secs: Option<u64>,
}
```

---

## 6. MVP 范围

### 第一版必须做（MVP）

1. **Cargo 依赖新增**：`opentelemetry`、`opentelemetry_sdk`、`opentelemetry-otlp`（grpc-tonic feature）、`opentelemetry-http`
2. **OTel 初始化**：启动时创建 `TracerProvider`，支持 OTLP gRPC 导出，批量导出模式
3. **配置读取**：`enabled`、`otlp_endpoint`、`service_name`、`sampling_ratio` 四个字段
4. **入站 traceparent 处理**：读取或生成，创建 root span（在 `request_filter`）
5. **上游 traceparent 注入**：在新增的 `upstream_request_filter` 中实现（W3C 格式）
6. **Span 结束**：在 `logging` hook 中 end()，携带最终状态码
7. **trace_id 与 access log 联动**：OTel trace_id 写回 `ctx.request_info.x_trace_id`，保证两者可关联

### 第一版不做（后续迭代）

- B3/Zipkin 格式兼容（非标准场景）
- 控制面 tracing-opentelemetry bridge（控制面追踪）
- gRPC 请求的 span 扩展属性（grpc.service、grpc.method）
- 自定义 span processor 或采样策略（ParentBased 已足够）
- 动态配置热更新（OTel 配置变更需重启）
- OpenTelemetry Baggage 传播

---

## 7. 下一步（第一个 PR 的 scope）

**PR 标题**：`feat(otel): add OpenTelemetry distributed tracing with OTLP export`

**范围**（与 MVP 一致）：

### 文件改动清单

| 文件 | 改动类型 | 说明 |
|------|---------|------|
| `Cargo.toml` | 新增依赖 | 4 个 OTel crate |
| `src/core/gateway/observe/mod.rs` | 新增子模块 | pub mod tracing |
| `src/core/gateway/observe/tracing/mod.rs` | 新建 | OTel 初始化、全局 TracerProvider、shutdown |
| `src/core/gateway/cli/config/otel.rs` | 新建 | OtelConfig 结构体及默认值 |
| `src/core/gateway/cli/config/mod.rs` | 修改 | 集成 OtelConfig 到 EdgionGatewayConfig |
| `src/core/gateway/cli/mod.rs` | 修改 | `run()` 中调用 `init_otel_tracer()` |
| `src/types/ctx.rs` | 修改 | EdgionHttpContext 新增 `otel_span` 字段 |
| `src/core/gateway/routes/http/proxy_http/pg_request_filter.rs` | 修改 | 入站 traceparent 读取 + root span 创建 |
| `src/core/gateway/routes/http/proxy_http/pg_upstream_request_filter.rs` | 新建 | 出站 traceparent 注入 + child span |
| `src/core/gateway/routes/http/proxy_http/mod.rs` | 修改 | 注册 `upstream_request_filter` hook |
| `src/core/gateway/routes/http/proxy_http/pg_upstream_response_filter.rs` | 修改 | 记录上游状态码到 span |
| `src/core/gateway/routes/http/proxy_http/pg_logging.rs` | 修改 | span.end() |

### 实现顺序建议

1. Cargo.toml + OtelConfig（配置层，不影响现有功能）
2. `observe/tracing/mod.rs`（初始化模块，含 `enabled` guard）
3. `cli/mod.rs` 注册初始化（`enabled=false` 时跳过，零影响）
4. `ctx.rs` 新增字段（`Option<Span>` 默认 None，向后兼容）
5. `pg_request_filter.rs`（入站处理，`enabled=false` 时跳过）
6. `pg_upstream_request_filter.rs`（新 hook）
7. `pg_upstream_response_filter.rs` + `pg_logging.rs`（收尾）

### 验证方式

1. 本地启动 Jaeger all-in-one（`docker run -p 4317:4317 -p 16686:16686 jaegertracing/all-in-one`）
2. 配置 `otel.enabled=true`，`otlp_endpoint=http://localhost:4317`
3. 发送请求，在 Jaeger UI 验证 trace 出现
4. 检查 access log 的 `x-trace-id` 与 Jaeger trace_id 一致
5. 验证上游服务收到 `traceparent` 头（用 echo 服务）

---

## 附：关键风险点

| 风险 | 描述 | 缓解措施 |
|------|------|---------|
| Retry 场景 Span 管理 | `Span` 不可 Clone，retry 时 ctx 被 clone | root span 用 `Arc<Mutex<Span>>`，或 retry 时重新开 child span |
| 热路径性能 | OTel SDK 批量导出，但 span 创建有开销 | `enabled=false` 时完全跳过所有 OTel 逻辑；采样率可调 |
| OTLP 连接断开 | 导出失败不应影响请求 | OTel SDK 批量导出自带缓冲和重试，失败静默丢弃 |
| trace_id 格式变化 | OTel trace_id 是 128-bit hex，现有 x-trace-id 是 UUID | 两者都是 32 hex char，格式兼容；access log 侧无感知 |
| 依赖冲突 | opentelemetry-otlp 的 grpc-tonic 与项目现有 tonic 0.12 | 两者兼容，需 lockfile 验证 |
