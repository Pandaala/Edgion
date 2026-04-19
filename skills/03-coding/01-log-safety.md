# 日志安全规范

> 三条铁律：不泄密、不泄配置、数据面不打 tracing。

## 铁律 1：敏感信息不入日志

### 绝对禁止

| 类别 | 示例 | 处理方式 |
|------|------|---------|
| Token / API Key / Secret 原文 | JWT token, HMAC key, API key | 只记录前 8 字符 + `***`，或仅记录 hash |
| 密码 / 证书私钥 | TLS private key, DB password | 完全不记录，只记录操作结果（成功/失败） |
| 证书全文 | PEM 内容 | 只记录 fingerprint / subject / SAN |
| 用户 PII | 姓名、邮箱、手机号 | 不记录，或脱敏（e.g. `j***@example.com`） |
| 完整请求/响应 body | POST body, response payload | 只记录 Content-Length / Content-Type |
| ACME challenge token | HTTP-01 token | 完全不记录到 tracing，仅通过 access log 记录请求路径 |

### 需要审慎处理

| 类别 | 风险 | 建议 |
|------|------|------|
| `ctx_map` (access log) | 插件可能存入 `jwt_claims`, `oidc_claims` 等 | 序列化前做 key 过滤/allowlist |
| `client_cert_info` | subject / SAN 可能包含 PII | mTLS 场景下审查 access log 输出 |
| `X-Forwarded-For` | 客户端真实 IP | 视合规要求决定是否保留 |
| plugin `stage_logs` | 可能包含 username | 插件应只记录 `u=<prefix>***` |

### 代码示例

```rust
// ✅ 安全
tracing::info!(kind = "Secret", name = %name, "Secret loaded successfully");
log.push(&format!("OK k={}***; ", &api_key[..api_key.len().min(8)]));

// ❌ 危险
tracing::info!(secret_data = %secret.data, "Secret content");
tracing::info!(token = %full_token, "ACME challenge token");
log.push(&format!("OK token={}; ", full_jwt_token));
```

### PluginLog 错误条目：必须用 `push_err`

> **API 规范 + 示例**在 [`observability/00-access-log.md`](observability/00-access-log.md) 的「错误条目 API 约束」小节（权威来源）。本节只讲**为什么这么约束**。

**强制规则**：插件错误路径一律走 `plugin_log.push_err(kind: &'static str)` 或 `u_push_err`，**禁止** `plugin_log.push(&format!("...", err))` / `push(&err.to_string())` / `push(any_dynamic_string)`。

**为什么 `&'static str` 签名**：

1. **编译期拦截 `format!` / `to_string()` / 局部 `&str`** —— 绝大多数泄漏路径（`jsonwebtoken::Error` 的 Display 可能带 token 片段、`reqwest::Error` 可能带完整 URL、claim 值可能被 format 进字符串）在 call site 就过不了编译，不需要靠 review 肉眼抓。

2. **只允许字面量和 `const`**（技术上 `Box::leak(format!(...).into_boxed_str())` 也通过，但这种写法在 review 时 grep 一眼能抓；不构成实际威胁）。

3. **强迫开发者先 `match` 再写标签**：`jwt_err.kind()` → 一组固定的静态字符串。分类动作被前置到代码里，从源头保证 kind 是 bounded set，也方便后续 access log 聚合按 kind 聚类。

4. **零堆分配**：LogBuffer 是 100B SmallVec，`push_err` 通过 3 段 `extend_from_slice(&'static [u8])` 写入，没有中间 String 分配 —— 数据面热路径可用。

**禁止同时双写**：同一错误只在 `push_err`（分类）或 `ctx.err_log`（脱敏后的详情）二选一。两者都写等于复制敏感内容两份。

---

## 铁律 2：配置不泄漏到日志

### 不应在 info 级别打印的内容

| 内容 | 日志级别 | 原因 |
|------|---------|------|
| 完整 resource spec | debug/trace only | 可能包含 Secret 引用、内部地址 |
| 后端 IP:Port 列表 | debug only | 内部网络拓扑 |
| 路由规则详情 | debug only | 业务路由逻辑 |
| 插件配置 JSON | debug only | 可能包含密钥配置 |
| 健康检查地址 | debug only | 内部端点暴露 |

### 允许在 info 级别打印

| 内容 | 示例 |
|------|------|
| 资源 key_name | `namespace/name` |
| 资源数量统计 | `count=42` |
| 事件类型 | `add` / `update` / `delete` |
| rv / sv | 版本号 |
| 错误摘要 | 错误类型 + 简短描述 |

### 代码示例

```rust
// ✅ info 级别：只记录 key 和统计
tracing::info!(kind = "HTTPRoute", key = %key, rv, "Resource processed");
tracing::info!(kind = "EdgionTls", count = data.len(), "Full set completed");

// ❌ info 级别：泄漏了完整配置
tracing::info!(spec = ?route.spec, "Route spec details");
tracing::info!(backend = ?backend_ref, "Selected backend");  // 包含 IP/Port
```

---

## 铁律 3：数据面不打 tracing 日志

### 原则

数据面（请求/连接处理路径）只通过以下方式产生日志：

| 方式 | 用途 | 示例 |
|------|------|------|
| **access log** | 请求级结果记录 | `AccessLogEntry` / `log_tls()` |
| **ctx.err_log / ctx.log** | 连接级错误/事件 | TLS proxy 的 `ctx.err_log = Some(...)` |
| **PluginLog** | 插件执行摘要 | `log.push("OK u=alice; ")` |
| **Metrics** | 聚合计数 | `global_metrics().request_success()` |

**禁止在请求/连接处理热路径中使用任何 `tracing::` 宏**，包括 `debug!`。

**全局禁止 `println!` / `eprintln!`**：生产代码中不允许使用 `println!` 或 `eprintln!`，必须使用 `tracing::` 宏并带 `component` 字段。启动阶段致命错误用 `tracing::error!` + `std::process::exit(1)`。

### 原因

1. **性能**：tracing 在高并发下有 lock contention（`debug!` 虽然可通过 `EnvFilter` 过滤，但 format 开销仍存在）
2. **日志膨胀**：每请求/连接一条 tracing log × QPS = 巨量日志
3. **信息重复**：access log / ssl log / tcp log 已经覆盖请求/连接维度信息
4. **关联性**：tracing log 无法关联到具体请求，access log 可以

### 数据面入口清单（判定边界）

> **判定法则**：从下表任一入口出发的调用链**全部**是数据面热路径，tracing 一律禁止（仅 ctx/access log/metrics）。
> 入口之外的代码（构造、装配、同步、启动、后台 worker）按各自上下文判断，多数可用 tracing。

**HTTP 请求路径** — `impl ProxyHttp for EdgionHttpProxy` (`gateway/routes/http/proxy_http/mod.rs`)

| 回调 | 分派文件 |
|------|----------|
| `new_ctx` / `init_downstream_modules` | `pg_new_ctx.rs` / `pg_init_downstream_modules.rs` |
| `early_request_filter` / `request_filter` / `request_body_filter` | `pg_early_request_filter.rs` / `pg_request_filter.rs` / `pg_request_body_filter.rs` |
| `upstream_peer` / `connected_to_upstream` | `pg_upstream_peer.rs` / `pg_connected_to_upstream.rs` |
| `upstream_response_filter` / `upstream_response_body_filter` / `response_filter` | `pg_upstream_response_filter.rs` / `pg_upstream_response_body_filter.rs` / `pg_response_filter.rs` |
| `fail_to_connect` / `error_while_proxy` / `fail_to_proxy` / `logging` | `pg_fail_to_connect.rs` / `pg_error_while_proxy.rs` / `pg_fail_to_proxy.rs` / `pg_logging.rs` |

HTTP→HTTPS redirect 服务（`routes/http/redirect_http.rs` 的 `ProxyHttp` impl）同等。

**TCP/TLS 连接路径** — `impl ServerApp` 的 `process_new`

| 文件 | 场景 |
|------|------|
| `gateway/routes/tcp/edgion_tcp.rs` | TCP |
| `gateway/routes/tls/passthrough_proxy.rs` | TLS 透传 |
| `gateway/routes/tls/proxy.rs` | TLS 卸载 + TCP 转发 |

**UDP 连接路径**（**非** Pingora trait，自建循环，每 packet 入口）— `gateway/routes/udp/edgion_udp.rs`
- `EdgionUdpProxy::serve()` 的 `recv_from` 循环
- `handle_client_packet` / `handle_upstream_packets`（tokio::spawn 起的 per-session 处理）

**TLS 握手回调** — `impl TlsAccept for TlsCallback` (`gateway/tls/runtime/gateway/tls_pingora.rs`)
含 `certificate_callback` / `handshake_complete_callback`。

**BoringSSL 底层回调**（TLS 握手同步调用链内）：
- `tls/boringssl/mtls_verify_callback.rs` 的 `extern "C" fn verify_callback` / `verify_callback_impl`（mTLS 白名单校验，每握手触发）
- `tls/runtime/backend/cert_extractor.rs`（`handshake_complete_callback` 中提取客户端证书）

**插件派发**（作为上述回调的延伸，全部算数据面）：
- `plugins/runtime/plugin_runtime.rs` 中 `run_*_plugins`
- `plugins/runtime/traits/**` 下 `RequestFilter` / `UpstreamResponse*` 实现
- `plugins/runtime/conditional_filter.rs`、`plugins/runtime/session_adapter.rs`、`plugins/runtime/gateway_api_filters/**`
- `plugins/http/**/plugin.rs` 的 `filter` / `apply` / `on_request` 方法
- `plugins/stream/stream_plugin_runtime.rs`、`plugins/stream/connection_filter_bridge.rs`（`impl ConnectionFilter`）、`plugins/stream/*/plugin.rs`（`impl StreamPlugin`）
- `plugins/stream/tls_route/tls_route_plugin_runtime.rs`、`plugins/stream/tls_route/*.rs`（`impl TlsRoutePlugin`，TLS 握手后的路由插件）

**从回调链进入的共享模块**（调用时=数据面，仅配置/启动时调用的不是）：
- `routes/http/match_engine/{radix_route_match,regex_routes_engine}.rs`、`routes/http/match_unit.rs`（HTTP 路由匹配，`request_filter` 调用）
- `routes/grpc/{match_engine,match_unit,integration}.rs`（gRPC 匹配，沿用 HTTP proxy 的 `request_filter`）
- `routes/http/session_persistence.rs`（`upstream_peer` 粘性会话）
- `lb/selection/{round_robin,least_conn,ewma,consistent_hash}.rs`、`lb/runtime_state/mod.rs`（`upstream_peer` 每次选 backend）
- `backends/mod.rs` peer 选择、`backends/validation.rs` SSRF 校验、`backends/circuit_breaker/store.rs` 熔断判定、`backends/policy/` 每请求策略
- `cache/**` 每请求 lookup
- `tls/store/cert_matcher.rs` 的 SNI→证书匹配（被 `certificate_callback` 调用）
- `runtime/server/error_response.rs`（`fail_to_proxy`/`request_filter` 拒绝时生成响应）

#### 明确**不是**数据面（即使在 `gateway/` 目录下）

| 位置 | 原因 |
|------|------|
| `plugins/**/conf_handler_impl.rs`、`PluginRuntime::build_*` | 配置/同步期构造插件，一次性 |
| `plugins/**::new()` / `::from_config()` | 构造器，非请求路径 |
| `conf_sync/**`、`link_sys/**` | Controller↔Gateway gRPC，与请求无关 |
| `api/**` | Admin API（控制面） |
| `runtime/server/**` 启动/关停 | 一次性事件 |
| `observe/metrics/registry.rs` 注册 | 启动期 |
| `services/acme/**` 后台任务、`backends/health/` 健康探测 loop、`backends/discovery/` 周期任务 | 后台 worker，按 job 频率而非 QPS |
| `tls/store/**` 证书加载、`tls/validation/` 解析 | 配置加载 |

### 例外

| 场景 | 允许 | 原因 |
|------|------|------|
| 配置加载/变更（`ConfHandler`/`conf_handler_impl`） | `tracing::info!` | 不在请求路径，属于系统事件 |
| 数据面启动/关闭（进程级 `once_cell` 初始化） | `tracing::info!` | 一次性事件 |
| 致命错误（panic 前） | `tracing::error!` | 极端情况需要立即可见 |
| `pg_logging.rs` 中 access log 发送失败 | `tracing::warn!` | log store 问题需要系统级可见性 |
| `mtls_verify_callback.rs` 的 `catch_unwind` panic 捕获分支 | `tracing::error!` | C callback 内 Rust panic 会 UB，必须立即可见 |
| `proxy_http/parse_timeout.rs`（`ParsedTimeouts::from_config`） | `tracing::warn!` | 仅在 listener 构建期解析 `EdgionGatewayConfig.httpTimeout` 字符串时调用，每次 reload 一次；文件虽在 `proxy_http/` 目录下，但不在请求路径 |
| `plugins/http/basic_auth/plugin.rs`（`load_users` warn + 构造器 Loaded info） | `tracing::warn!` / `tracing::info!` | 仅 `BasicAuth::new` 调用的 htpasswd 解析 / 重复 username 去重 / Loaded 汇总，只在配置加载期触发 |
| `plugins/http/basic_auth/plugin.rs`（bcrypt `spawn_blocking` panic 捕获分支） | `tracing::warn!` | tokio JoinError 极低频，类比 `mtls_verify_callback` 的 panic 捕获；不在每请求热路径 |
| `plugins/http/header_cert_auth/plugin.rs`（`load_ca_certs` 的 drop expired / unparseable CA） | `tracing::warn!` | CA 信任池仅在插件构造期装配，drop 决策不参与每请求验证路径 |

### 代码示例

```rust
// ✅ TLS proxy: 使用 ctx-based logging
ctx.err_log = Some(format!("No matching TLSRoute for SNI={sni}"));

// ✅ HTTP proxy: 错误进入 access log
ctx.add_error(EdgionStatus::NoRoute);

// ✅ 插件: 通过 PluginLog
log.push("Deny rate-limited; ");

// ❌ 数据面热路径: 不要用 tracing
tracing::warn!("No route found for {}", host);  // 每请求都打，日志爆炸
tracing::info!("Selected backend: {:?}", backend_ref);  // 泄漏 + 性能
```

---

## 检查清单

在 code review 时使用：

- [ ] 是否使用了 `println!` / `eprintln!`？必须改为 `tracing::` 宏
- [ ] 新增的 `tracing::info!/warn!/error!` 是否在数据面热路径？如果是，改为 ctx-based
- [ ] 日志中是否包含 Secret / Token / 密钥等敏感数据？
- [ ] 日志中是否包含完整的 resource spec / backend 地址？如果是，降级到 debug
- [ ] 控制面新增日志是否包含 `kind` + `name/namespace` + `rv`？（在 span 内可省略，由 span 自动提供）
- [ ] 如果资源已同步到数据面，access log 中是否有 `sv`？
- [ ] `ctx_map` 中新增的 key 是否可能包含敏感信息？如果是，需要做过滤
- [ ] 插件 stage_logs 中是否记录了完整 username / token？应只记录前缀

---

## 定期维护（每季度或每次改「数据面入口清单」后执行）

**1. 扫描数据面入口文件是否混入新 tracing**（注意：只点**入口文件本身**，目录里那些 `conf_handler_impl.rs` / `routes_mgr.rs` / `conf_handler.rs` 是配置期，不要整目录扫）：

```bash
rg -n 'tracing::' \
  src/core/gateway/routes/http/proxy_http/ \
  src/core/gateway/routes/http/match_engine/ \
  src/core/gateway/routes/http/match_unit.rs \
  src/core/gateway/routes/http/session_persistence.rs \
  src/core/gateway/routes/http/redirect_http.rs \
  src/core/gateway/routes/tcp/edgion_tcp.rs \
  src/core/gateway/routes/tls/passthrough_proxy.rs \
  src/core/gateway/routes/tls/proxy.rs \
  src/core/gateway/routes/udp/edgion_udp.rs \
  src/core/gateway/routes/grpc/match_engine.rs \
  src/core/gateway/routes/grpc/match_unit.rs \
  src/core/gateway/routes/grpc/integration.rs \
  src/core/gateway/tls/runtime/gateway/tls_pingora.rs \
  src/core/gateway/tls/runtime/backend/backend_api.rs \
  src/core/gateway/tls/runtime/backend/cert_extractor.rs \
  src/core/gateway/tls/boringssl/mtls_verify_callback.rs \
  src/core/gateway/tls/store/cert_matcher.rs \
  src/core/gateway/plugins/stream/stream_plugin_runtime.rs \
  src/core/gateway/plugins/stream/connection_filter_bridge.rs \
  src/core/gateway/plugins/stream/tls_route/ \
  src/core/gateway/plugins/stream/ip_restriction/ \
  src/core/gateway/plugins/stream/global_connection_ip_restriction/ \
  src/core/gateway/plugins/runtime/plugin_runtime.rs \
  src/core/gateway/plugins/runtime/conditional_filter.rs \
  src/core/gateway/plugins/runtime/session_adapter.rs \
  src/core/gateway/plugins/runtime/gateway_api_filters/ \
  src/core/gateway/plugins/http/*/plugin.rs \
  src/core/gateway/plugins/http/*/request_filter_impl.rs \
  src/core/gateway/lb/selection/ \
  src/core/gateway/lb/runtime_state/ \
  src/core/gateway/backends/mod.rs \
  src/core/gateway/backends/validation.rs \
  src/core/gateway/backends/circuit_breaker/store.rs \
  src/core/gateway/backends/policy/ \
  src/core/gateway/cache/ \
  src/core/gateway/runtime/server/error_response.rs
```

**预期**：只剩下例外表登记的豁免项（`pg_logging.rs` 的 access log 失败 warn、`mtls_verify_callback.rs` 的 panic 捕获 error 等）。任何新命中必须：
- 改为 ctx-based log / access log / PluginLog / metrics，**或**
- 评审后补入例外表并注明原因。

> **当前基线**（2026-04-19）：Wave 4 工单 #03–#13 尚未执行完，上面命令**预期**会命中 `pg_*.rs` / `mtls_verify_callback.rs` init log / `lb/runtime_state/mod.rs` / `lb/selection/round_robin.rs` / `match_engine/radix_*.rs` 等位置。这些属于存量债务，应通过 Wave 4 清空，而非新豁免。
> `parse_timeout.rs` 是配置加载路径（`ParsedTimeouts::from_config` 仅 listener 构建时调用），已登记到上面的例外表，保留合理。

**2. 核对 Pingora / 插件 trait impl 有没有新增没登记的入口**：

```bash
# 数据面 trait impl —— 每个结果的文件路径必须能在"数据面入口清单"表或"插件派发"小节里对得上
rg -n 'impl (ProxyHttp|ServerApp|TlsAccept|ConnectionFilter|StreamPlugin|TlsRoutePlugin|RequestFilter|UpstreamResponseFilter|UpstreamResponseBodyFilter|UpstreamResponse) for ' \
  src/core/gateway/
```

**预期**：每条命中都能在本规则的"HTTP 请求路径 / TCP/TLS 连接路径 / UDP 连接路径 / TLS 握手回调 / 插件派发"这几节里找到覆盖。新增 trait impl 必须同步更新上面表格，否则下次 review 工单判定会漏。

---

## 已知待治理项

以下是存量代码中已识别的问题，在改动相关文件时应顺带修复：

| 文件 | 问题 | 修复方式 | 状态 |
|------|------|---------|------|
| ~~HTTP proxy 全路径~~ | ~~`pg_upstream_peer` 等多个文件存在 tracing~~ | ~~全部删除~~ | ✅ 已修复 |
| ~~`tls_pingora.rs`~~ | ~~TLS handshake 路径大量 tracing~~ | ~~全部删除~~ | ✅ 已修复 |
| ~~`edgion_tcp.rs`~~ | ~~TCP 连接路径 tracing~~ | ~~全部删除~~ | ✅ 已修复 |
| ~~`stream_plugin_runtime.rs`~~ | ~~stream plugin deny 的 info log~~ | ~~删除~~ | ✅ 已修复 |
| ~~`stream_ip_restriction.rs`~~ | ~~连接级 debug/info~~ | ~~全部删除~~ | ✅ 已修复 |
| ~~`connection_filter_bridge.rs`~~ | ~~连接过滤 debug/info~~ | ~~全部删除~~ | ✅ 已修复 |
| ~~`grpc/match_unit.rs`~~ | ~~请求级 warn~~ | ~~删除~~ | ✅ 已修复 |
| ~~`preflight_handler.rs`~~ | ~~preflight debug~~ | ~~删除~~ | ✅ 已修复 |
| access log `ctx_map` | 完整 ctx_map 序列化，可能含 JWT/OIDC/JWE claims | 增加 key allowlist/blocklist（中期任务） | 待处理 |
| LDAP/JWT auth plugin | username 进入 stage_logs | 改为 `u=<prefix>***` 格式 | 待处理 |
