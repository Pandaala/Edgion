---
name: gateway-pingora-lifecycle
description: Pingora ProxyHttp 生命周期回调：8 阶段处理链、ConnectionFilter TCP 过滤、EdgionHttpContext 状态传递、Access Log 零拷贝生成。
---

# Pingora ProxyHttp 生命周期

`EdgionHttpProxy` (`routes/http/proxy_http/mod.rs`) 实现 Pingora 的 `ProxyHttp` trait，以 `EdgionHttpContext` 作为每请求上下文类型（`type CTX = EdgionHttpContext`）。每个回调阶段被拆分为独立子模块 `pg_*.rs` 以保持可维护性。

## ProxyHttp 8 阶段回调链

```
                    ┌──────────────────────────────┐
    TCP 连接 ──────>│  ConnectionFilter (TCP 层)    │──── 拒绝 ──> 关闭连接
                    └──────────┬───────────────────┘
                               │ 允许
                    ┌──────────▼───────────────────┐
                    │  TLS 握手 (如果 HTTPS)        │
                    └──────────┬───────────────────┘
                               │
                    ┌──────────▼───────────────────┐
              ① ──> │  early_request_filter         │──── ACME 命中 ──> 直接响应
                    └──────────┬───────────────────┘
                               │
              ② ──> ┌──────────▼───────────────────┐
                    │  request_filter               │──── 400/404/421 ──> 错误响应
                    └──────────┬───────────────────┘
                               │
              ③ ──> ┌──────────▼───────────────────┐
                    │  upstream_peer                │──── 504 ──> 超时
                    └──────────┬───────────────────┘
                               │
              ④ ──> ┌──────────▼───────────────────┐
                    │  connected_to_upstream        │
                    └──────────┬───────────────────┘
                               │
              ⑤ ──> ┌──────────▼───────────────────┐
                    │  upstream_response_filter     │ (sync)
                    └──────────┬───────────────────┘
                               │
              ⑥ ──> ┌──────────▼───────────────────┐
                    │  upstream_response_body_filter│ (sync, 逐 chunk)
                    └──────────┬───────────────────┘
                               │
              ⑦ ──> ┌──────────▼───────────────────┐
                    │  response_filter              │ (async)
                    └──────────┬───────────────────┘
                               │
              ⑧ ──> ┌──────────▼───────────────────┐
                    │  logging                      │
                    └──────────────────────────────┘
```

### 阶段 1：early_request_filter

**文件**：`pg_early_request_filter.rs`
**签名**：`async fn early_request_filter(session, ctx) -> Result<()>`

在 HTTP 头解析完成后、路由匹配之前执行：

1. **ACME HTTP-01 challenge**：检查全局 `ChallengeStore`（快速路径：`is_empty()` 检查在 99.99% 请求中零开销）。命中时直接发送 200 响应并短路返回 `HTTPStatus(200)` 错误终止代理流水线。
2. **客户端超时设置**：从预解析的 `ParsedTimeouts.client` 设置 `read_timeout` 和 `write_timeout`。
3. **Keepalive 管理**：正常时设置 keepalive_timeout；关闭中（`is_process_shutting_down()`）设为 None 禁用 keepalive 以辅助流量排空。

### 阶段 2：request_filter

**文件**：`pg_request_filter.rs`
**签名**：`async fn request_filter(session, ctx) -> Result<bool>`（返回 true 表示响应已发送）

最复杂的阶段，负责请求解析和路由匹配：

1. **元数据提取** (`build_request_metadata`)：
   - 协议检测：WebSocket（Upgrade header）、gRPC-Web/gRPC（Content-Type header）
   - 地址提取：`client_addr`（TCP 直连 IP）、`remote_addr`（经过 RealIpExtractor 处理的真实客户端 IP）
   - Hostname 归一化：URI host → Host header → :authority header，统一去端口、去尾点、转小写
   - TLS 元数据：从 `SslDigestExtension` 提取 `tls_id`、`sni`、`client_cert_info`
   - 安全校验：SNI-Host 一致性检查、Listener hostname 隔离、X-Forwarded-For 长度限制

2. **路由匹配**：
   - 从 `PortGatewayInfoStore` 动态加载当前端口的 GatewayInfo 列表
   - gRPC 请求优先尝试 GRPCRoute 匹配（`try_match_grpc_route`）
   - HTTP 请求通过 `port_routes.match_route()` 匹配（radix tree + regex 引擎）
   - 匹配失败返回 404

3. **Preflight 处理**：CORS preflight 请求在插件执行前处理，从匹配路由的 EdgionPlugins 中提取 CORS 配置

4. **插件执行**（RequestFilter 阶段）：
   - 先执行全局插件（`EdgionGatewayConfig.spec.global_plugins_ref`）
   - 再执行路由级插件（`route_unit.rule.plugin_runtime.run_request_plugins()`）
   - 任一插件返回 `ErrTerminateRequest` 则终止请求

5. **Header 处理**：设置 `X-Real-IP`、追加 `X-Forwarded-For`

### 阶段 3：upstream_peer

**文件**：`pg_upstream_peer.rs`
**签名**：`async fn upstream_peer(session, ctx) -> Result<Box<HttpPeer>>`

选择上游后端并构建 HttpPeer：

1. **请求超时检查**：检查 route 级别的 `request_timeout`，超时返回 504
2. **路由分流**：gRPC 路由走 `upstream_peer_grpc()`，HTTP 路由走 `upstream_peer_http()`
3. **HTTP 后端选择**（按优先级）：
   - `DirectEndpoint`：插件指定的精确地址，绕过 LB
   - `ExternalJump`：外部域名，需 DNS 解析（拒绝 localhost）
   - `InternalJump`：按名称查找 BackendRef
   - 常规路径：`RouteRules::select_backend()` 加权轮询选择 BackendRef，然后 `get_peer()` 通过 LB 选择具体 endpoint
4. **BackendTLSPolicy 查询**：根据 Service namespace 查找 TLS 策略
5. **后端级插件执行**：`backend_ref.plugin_runtime.run_request_plugins()`
6. **Peer 配置**：
   - 超时：route 级 `backend_request_timeout` 覆盖全局 `request_timeout`，统一设置 connection/read/write timeout
   - 空闲超时：使用全局 `idle_timeout`
   - gRPC 强制 HTTP/2：`peer.options.set_http_version(2, 2)`
7. **指标更新**：递增 `try_cnt`，记录 upstream IP/port，设置 `upstream_start_time`

### 阶段 4：connected_to_upstream

**文件**：`pg_connected_to_upstream.rs`
**签名**：`async fn connected_to_upstream(session, reused, peer, fd, digest, ctx) -> Result<()>`

上游连接建立后回调：

1. **连接时间记录**：计算 `ct`（connect time，从上游尝试开始到连接建立的毫秒数）
2. **LeastConn 计数**：如果 LB 策略为 LeastConn，调用 `runtime_state::increment()` 增加活跃连接计数
3. **流式协议日志**：gRPC-Web 和 WebSocket 请求立即发送一条 `conn_est` AccessLog 条目

### 阶段 5：upstream_response_filter（同步）

**文件**：`pg_upstream_response_filter.rs`
**签名**：`fn upstream_response_filter(session, upstream_response, ctx) -> Result<()>`（注意：非 async）

接收到上游响应头时调用：

1. **状态码记录**：`ctx.request_info.status = status_code`
2. **Server Header**：通过 `ServerHeaderOpts::apply_to_response()` 注入自定义 Server header
3. **Header 时间记录**：计算 `ht`（header time，从上游尝试开始到收到响应头的毫秒数）
4. **同步插件执行**：
   - Rule 级：`route_unit.rule.plugin_runtime.run_upstream_response_plugins_sync()`
   - Backend 级：`selected_backend.plugin_runtime.run_upstream_response_plugins_sync()`

### 阶段 6：upstream_response_body_filter（同步，逐 chunk）

**文件**：`pg_upstream_response_body_filter.rs`
**签名**：`fn upstream_response_body_filter(session, body, end_of_stream, ctx) -> Result<Option<Duration>>`

每个上游响应体 chunk 调用一次，返回 `Some(Duration)` 实现带宽限流：

1. **Body 时间记录**：首个 chunk 时计算 `bt`（body time）
2. **路由级 body filter 插件**：带宽限流等
3. **全局 body filter 插件**：从 `EdgionGatewayConfig.spec.global_plugins_ref` 加载
4. **限流合并**：多个插件的 delay 取最大值

### 阶段 7：response_filter（异步）

**文件**：`pg_response_filter.rs`
**签名**：`async fn response_filter(session, upstream_response, ctx) -> Result<()>`

异步响应处理，可执行需要 await 的操作：

1. **Server Header**：再次应用（捕获框架生成的错误响应如 502）
2. **Trace ID 注入**：将 `x-trace-id` 添加到响应头
3. **优雅关闭**：关闭中添加 `Connection: close` 响应头
4. **异步插件执行**（UpstreamResponse 阶段）：
   - Rule 级：`run_upstream_response_plugins_async()`
   - Backend 级：`run_upstream_response_plugins_async()`

### 阶段 8：logging

**文件**：`pg_logging.rs`
**签名**：`async fn logging(session, error, ctx)`

请求完成后的收尾阶段：

1. **上游指标收集**：
   - 响应体大小：`session.upstream_body_bytes_received()`
   - 写入等待时间：`session.upstream_write_pending_time()`（HTTP/1.x 背压）
   - 全局带宽计数：`add_request_bytes()` + `add_response_bytes()`
2. **LB 运行时状态更新**：
   - LeastConn：`runtime_state::decrement()` 减少活跃连接数
   - EWMA：`runtime_state::update_ewma()` 更新延迟指标
3. **响应状态更新**：从 `session.response_written()` 获取最终响应状态码
4. **Prometheus 指标**：`record_backend_request()` 按 gateway/route/backend/protocol/status_group 维度记录
5. **AccessLog 生成**：`AccessLogEntry::from_context(ctx)` → `to_json()` → `access_logger.send()`
6. **Mirror 清理**：`ctx.mirror_state.take()` drop JoinHandle 分离镜像任务
7. **集成测试存储**：当 `integration_testing_mode` 启用且请求带 `access_log: test_store` header 时，存入 Access Log Store

## ConnectionFilter（TCP 层）

**文件**：`plugins/stream/connection_filter_bridge.rs`

`StreamPluginConnectionFilter` 在 TLS 握手和 HTTP 解析之前对原始 TCP 连接执行过滤：

- 实现 Pingora 的 `ConnectionFilter` trait 的 `should_accept(addr: Option<&SocketAddr>) -> bool`
- 通过 Gateway annotation `edgion.io/edgion-stream-plugins` 引用 `EdgionStreamPlugins` 资源
- 从全局 `StreamPluginStore`（ArcSwap）读取最新插件配置，支持热重载
- 执行 `StreamPluginRuntime.run(&ctx)` 得到 `StreamPluginResult::Allow` 或 `StreamPluginResult::Deny`
- 无地址信息时（`addr == None`）默认允许
- 无配置或无活跃插件时默认允许

典型用途：IP 黑白名单限制、地理位置过滤、连接速率限制。

## EdgionHttpContext：每请求状态载体

**定义**：`types/ctx.rs`

`EdgionHttpContext` 在 `new_ctx()` 创建，贯穿 ProxyHttp 所有阶段，最终在 `logging()` 阶段消费。

### 核心字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `start_time` | `Instant` | 请求开始时间 |
| `gateway_info` | `GatewayInfo` | 匹配的 Gateway 元数据 |
| `request_info` | `RequestInfo` | 请求元数据：client_addr、remote_addr、hostname、path、status、x_trace_id、sni、tls_id、listener_port、discover_protocol |
| `edgion_status` | `Vec<EdgionStatus>` | 错误码收集 |
| `route_unit` | `Option<Arc<HttpRouteRuleUnit>>` | 匹配的 HTTP 路由规则 |
| `grpc_route_unit` | `Option<Arc<GrpcRouteRuleUnit>>` | 匹配的 gRPC 路由规则 |
| `selected_backend` | `Option<HTTPBackendRef>` | LB 选中的 HTTP 后端 |
| `selected_grpc_backend` | `Option<GRPCBackendRef>` | LB 选中的 gRPC 后端 |
| `backend_context` | `Option<BackendContext>` | 后端服务信息 + 上游尝试历史（UpstreamInfo 列表） |
| `stage_logs` | `Vec<StageLogs>` | 插件阶段执行日志 |
| `plugin_running_result` | `PluginRunningResult` | 插件执行结果 |
| `try_cnt` | `u32` | 后端连接尝试次数 |
| `ctx_map` | `HashMap<String, String>` | 插件间通信的上下文变量 |
| `direct_endpoint` | `Option<DirectEndpointPreset>` | DirectEndpoint 插件设置的精确上游地址 |
| `internal_jump` | `Option<InternalJumpPreset>` | DynamicInternalUpstream 插件设置的内部跳转目标 |
| `external_jump` | `Option<ExternalJumpPreset>` | DynamicExternalUpstream 插件设置的外部跳转目标 |
| `mirror_state` | `Option<MirrorState>` | 请求镜像状态 |

### UpstreamInfo 时间指标

每次上游尝试创建一个 `UpstreamInfo`，记录细粒度时间：
- `ct`：Connect Time — 从尝试开始到连接建立
- `ht`：Header Time — 从尝试开始到收到响应头
- `bt`：Body Time — 从尝试开始到收到首个 body chunk
- `et`：Elapsed Time — 总耗时
- `wpt`：Write Pending Time — 上游写入背压时间（HTTP/1.x）

## Access Log：零拷贝 JSON

**定义**：`observe/access_log/entry.rs`

`AccessLogEntry` 通过借用 `EdgionHttpContext` 的字段（`&'a RequestInfo`、`&'a BackendContext`、`&'a [EdgionStatus]`、`&'a [StageLogs]`）构建，避免数据复制。

```rust
pub struct AccessLogEntry<'a> {
    pub timestamp: i64,                      // 毫秒时间戳
    pub request_info: &'a RequestInfo,        // 借用
    pub match_info: Option<RouteMatchInfo<'a>>, // HTTP 或 gRPC 匹配信息
    pub errors: &'a [EdgionStatus],           // 借用
    pub backend_context: Option<&'a BackendContext>, // 借用（含 upstreams 列表）
    pub stage_logs: &'a [StageLogs],          // 借用
    pub conn_est: Option<bool>,               // WebSocket/gRPC-Web 连接建立标记
    pub ctx: &'a HashMap<String, String>,     // 插件上下文变量
}
```

通过 `serde_json` 的 `to_string()` 直接序列化为 JSON 字符串，单次分配输出。`#[serde(skip_serializing_if)]` 注解省略空字段以减小日志体积。

## 错误响应

Gateway 提供标准化的错误响应生成函数，均定义在 `runtime/server/error_response.rs`：

| 函数 | 状态码 | 触发场景 |
|------|--------|---------|
| `end_response_400` | 400 | Hostname 缺失、XFF header 超长 |
| `end_response_404` | 404 | 路由匹配失败 |
| `end_response_421` | 421 | SNI-Host 不匹配、Listener hostname 隔离失败 |
| `end_response_500` | 500 | 内部错误、后端选择失败、插件终止 |
| `end_response_503` | 503 | 服务不可用 |

所有错误响应都通过 `ServerHeaderOpts` 注入自定义 Server header。
