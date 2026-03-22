# 请求延迟 Histogram 指标 — 研究报告

## 1. 当前状态

### 1.1 延迟相关代码位置

**计时起点**：`src/types/ctx.rs` 第 379 行
```rust
pub struct EdgionHttpContext {
    pub start_time: Instant,   // 在 EdgionHttpContext::new() 中初始化为 Instant::now()
    ...
}
```
`start_time` 在 Pingora 的 `new_ctx()` hook 被调用时设置，即请求到达 gateway 的最早时刻。

**`upstream_start_time`**：同文件第 426 行，记录第一次向上游发起连接的时刻，用于计算纯后端响应时间（不含 request_filter 插件耗时）。

**UpstreamInfo 中的细粒度计时**（`src/types/ctx.rs` 第 209-263 行）：
- `ct`：connect time（毫秒，从 start_time 到连接建立）
- `ht`：header time（毫秒，从 start_time 到收到响应头）
- `bt`：body time（毫秒，从 start_time 到收到第一块 body）
- `et`：elapsed time（毫秒，本次 upstream 尝试的总时间）

这些字段已记录进 access log，但**均未写入任何 Prometheus Histogram**。

**access log 中的 latency_ms**：`src/core/gateway/observe/access_log/entry.rs` 第 82 行
`to_combined()` 方法接受外部传入的 `latency_ms: u64` 参数，说明调用方负责计算延迟。查看调用侧可知计算方式为 `ctx.start_time.elapsed().as_millis() as u64`（在 `pg_logging.rs` 或其上层调用时计算）。

**唯一的已有 Histogram**：`edgion_mirror_duration_ms`（`src/core/gateway/observe/metrics/registry.rs` 第 297-304 行），在 mirror 请求完成后调用 `histogram!(names::MIRROR_DURATION_MS).record(duration_ms)`，无任何 label。

### 1.2 指标记录的位置（Pingora hook）

`src/core/gateway/routes/http/proxy_http/pg_logging.rs` 的 `logging()` 函数对应 Pingora 的 `logging` hook，是整个请求生命周期中**最后一个被调用的 hook**（无论成功/失败/重试均会执行）。这里已经：
- 从 session 读取最终响应状态（`session.response_written()`）
- 调用 `record_request_metrics(ctx, _e)` 记录 Counter 指标
- 构建并发送 access log

**这是记录 end-to-end 延迟 Histogram 的最佳位置**，所有信息（route、backend、status、start_time）都已就绪。

### 1.3 现有 metrics 基础设施

- `metrics` crate 版本：`0.24`，`metrics-exporter-prometheus` 版本：`0.18`（均已在 `Cargo.toml` 依赖中）
- `histogram!` macro 已被使用（mirror 的例子），项目已验证可用
- 带 label 的 counter 用法已有模板（`record_backend_request` 函数），可直接参考

---

## 2. 实现路径

### 2.1 分步骤实现方案

#### Step A：在 `registry.rs` 中添加指标名称常量

文件：`src/core/gateway/observe/metrics/registry.rs`

新增常量（在 `names` mod 中）：
```
GATEWAY_REQUEST_DURATION_MS = "edgion_gateway_request_duration_ms"
```

无需在 `GatewayMetrics` struct 中添加字段——Histogram 与 Counter 不同，不需要预先注册句柄，直接用 `histogram!` macro 在调用时动态创建即可（与 `record_mirror_metric` 的做法一致）。

#### Step B：新增 `record_request_duration` 函数

文件：`src/core/gateway/observe/metrics/registry.rs`

参考 `record_backend_request` 和 `record_mirror_metric` 的模式，新增一个独立的函数：

```rust
pub fn record_request_duration(
    gateway_ns: &str,
    gateway_name: &str,
    route_ns: &str,
    route_name: &str,
    backend_ns: &str,
    backend_name: &str,
    protocol: &str,
    status: &str,
    duration_ms: f64,
) {
    histogram!(
        names::GATEWAY_REQUEST_DURATION_MS,
        "gateway_namespace" => gateway_ns.to_string(),
        "gateway_name" => gateway_name.to_string(),
        "route_namespace" => route_ns.to_string(),
        "route_name" => route_name.to_string(),
        "backend_namespace" => backend_ns.to_string(),
        "backend_name" => backend_name.to_string(),
        "protocol" => protocol.to_string(),
        "status" => status.to_string(),
    )
    .record(duration_ms);
}
```

#### Step C：在 `pg_logging.rs` 的 `record_request_metrics` 中调用

文件：`src/core/gateway/routes/http/proxy_http/pg_logging.rs`，`record_request_metrics` 函数末尾（当前第 156-168 行之后）。

调用时计算延迟：
```rust
let duration_ms = ctx.start_time.elapsed().as_secs_f64() * 1000.0;
record_request_duration(
    gateway_ns, gateway_name,
    route_ns, route_name,
    backend_ns, backend_name,
    protocol,
    status_group(ctx.request_info.status),
    duration_ms,
);
```

#### Step D（可选，后续 PR）：配置 Prometheus exporter 的 bucket 边界

文件：`src/core/gateway/observe/metrics/api.rs`（或 metrics exporter 的初始化位置）

通过 `PrometheusBuilder` 的 `set_buckets_for_metric` API 配置自定义 bucket。

---

## 3. 新增指标清单

### 3.1 主请求延迟 Histogram

| 属性 | 值 |
|------|-----|
| 名称 | `edgion_gateway_request_duration_ms` |
| 类型 | Histogram |
| 单位 | 毫秒（ms） |
| 计时区间 | `ctx.start_time`（new_ctx） → `logging` hook 执行时（response 已写入完毕） |

**Labels**：

| Label | 说明 | 示例值 |
|-------|------|--------|
| `gateway_namespace` | Gateway 所在命名空间 | `default` |
| `gateway_name` | Gateway 名称 | `prod-gateway` |
| `route_namespace` | 匹配 Route 的命名空间 | `default` |
| `route_name` | 匹配 Route 名称 | `api-route` |
| `backend_namespace` | 后端 Service 命名空间 | `default` |
| `backend_name` | 后端 Service 名称 | `api-svc` |
| `protocol` | 请求协议 | `http` / `grpc` / `websocket` |
| `status` | 响应状态组 | `2xx` / `3xx` / `4xx` / `5xx` / `failed` |

**Bucket 边界**（参考 Prometheus 最佳实践，适合 API Gateway 场景）：

```
[1, 5, 10, 25, 50, 100, 200, 500, 1000, 2500, 5000, 10000]  // 单位：ms
```

理由：
- 1ms~10ms：覆盖极快的本地服务或缓存命中
- 25ms~100ms：正常 LAN 内部调用
- 200ms~500ms：可接受的 API 响应
- 1000ms~5000ms：超时边界附近，P99 告警阈值通常在此范围
- 10000ms：兜底 bucket，捕获长尾

### 3.2 后续可扩展指标（本次 PR 不做）

- `edgion_upstream_connect_duration_ms`：纯后端连接时间（ct），label 增加 upstream IP
- `edgion_upstream_response_header_duration_ms`：首字节时间（ht）
- gRPC 路由专属的 `edgion_grpc_request_duration_ms`（可与 HTTP 合并用 protocol label 区分）

---

## 4. 关键代码位置

| 文件 | 行号范围 | 说明 |
|------|----------|------|
| `src/core/gateway/observe/metrics/registry.rs` | 10-38 | `names` mod，新增常量在此 |
| `src/core/gateway/observe/metrics/registry.rs` | 295-304 | `record_mirror_metric`，histogram 用法参考模板 |
| `src/core/gateway/observe/metrics/registry.rs` | 304 末尾 | **新增** `record_request_duration` 函数 |
| `src/core/gateway/routes/http/proxy_http/pg_logging.rs` | 108-168 | `record_request_metrics`，**在此函数末尾** 调用 `record_request_duration` |
| `src/core/gateway/routes/http/proxy_http/pg_logging.rs` | 49 | `record_request_metrics` 的调用点，延迟计算需在 `start_time` 到此处的 elapsed |
| `src/types/ctx.rs` | 379 | `start_time: Instant` 字段定义 |
| `src/types/ctx.rs` | 426 | `upstream_start_time: Option<Instant>`，可用于上游专属延迟 |

---

## 5. 关键问题解答

**请求结束时的 hook 在哪里？**
Pingora trait 的 `logging` hook，对应 `pg_logging.rs` 的 `logging()` 函数。该函数在 `mod.rs` 的 `ProxyHttp` impl 中被调用。这是唯一一个无论成功/失败都会执行的 hook，且 response 已经发送给客户端，是计算 end-to-end 延迟的最晚（也最准确）时机。

**latency_ms 的计算方式？**
`ctx.start_time`（`Instant::now()` 在 `new_ctx` hook 中设置）到 `logging` hook 执行时的 `elapsed()`。`access log entry` 的 `to_combined()` 方法接受外部计算好的 `latency_ms`，说明调用方（pg_logging.rs）负责计算并传入。

**`histogram!` macro 已在项目中使用吗？**
是，`record_mirror_metric` 函数中已有 `histogram!(names::MIRROR_DURATION_MS).record(duration_ms)` 的用法，且 `metrics-exporter-prometheus = "0.18"` 也已在 `Cargo.toml` 中声明，无需额外依赖变更。

**延迟需要哪些维度的标签？**
参考已有的 `record_backend_request` 函数的 label 设计：`gateway_namespace`、`gateway_name`、`route_namespace`、`route_name`、`backend_namespace`、`backend_name`、`protocol`、`status`。与 counter 保持一致，方便在 Grafana 中做 join/关联查询。

**最合适记录 end-to-end 延迟的位置？**
`pg_logging.rs` 的 `record_request_metrics` 函数末尾。此时：response status 已从 session 读取并写入 ctx（第 44-46 行），所有 route/backend 信息已就绪，`ctx.start_time` 可直接 `.elapsed()` 得到完整的客户端视角延迟。

---

## 6. 下一步：第一个 PR 的 scope

**目标**：只做 end-to-end 请求延迟的 Histogram，scope 最小。

**PR 包含的改动（共 2 个文件）**：

1. **`src/core/gateway/observe/metrics/registry.rs`**
   - 在 `names` mod 中新增常量 `GATEWAY_REQUEST_DURATION_MS`
   - 新增 `record_request_duration(...)` 函数（参照 `record_mirror_metric` 的模式，带 8 个 label）

2. **`src/core/gateway/routes/http/proxy_http/pg_logging.rs`**
   - 在 `record_request_metrics` 函数末尾，计算 `ctx.start_time.elapsed()` 并调用 `record_request_duration`

**不在本 PR 内**：
- bucket 边界自定义配置（exporter 初始化修改，单独 PR）
- upstream 专属延迟指标（`ct`/`ht` 等，单独 PR）
- gRPC 路由专属处理（本次通过 protocol label 复用同一指标）

**验证方式**：`curl :5901/metrics | grep edgion_gateway_request_duration_ms`，应能看到 `_bucket`、`_sum`、`_count` 三组时序数据。
