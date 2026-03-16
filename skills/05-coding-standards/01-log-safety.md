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

### 原因

1. **性能**：tracing 在高并发下有 lock contention（`debug!` 虽然可通过 `EnvFilter` 过滤，但 format 开销仍存在）
2. **日志膨胀**：每请求/连接一条 tracing log × QPS = 巨量日志
3. **信息重复**：access log / ssl log / tcp log 已经覆盖请求/连接维度信息
4. **关联性**：tracing log 无法关联到具体请求，access log 可以

### 例外

| 场景 | 允许 | 原因 |
|------|------|------|
| 配置加载/变更（`ConfHandler`/`conf_handler_impl`） | `tracing::info!` | 不在请求路径，属于系统事件 |
| 数据面启动/关闭（进程级 `once_cell` 初始化） | `tracing::info!` | 一次性事件 |
| 致命错误（panic 前） | `tracing::error!` | 极端情况需要立即可见 |
| `pg_logging.rs` 中 access log 发送失败 | `tracing::warn!` | log store 问题需要系统级可见性 |

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

- [ ] 新增的 `tracing::info!/warn!/error!` 是否在数据面热路径？如果是，改为 ctx-based
- [ ] 日志中是否包含 Secret / Token / 密钥等敏感数据？
- [ ] 日志中是否包含完整的 resource spec / backend 地址？如果是，降级到 debug
- [ ] 控制面新增日志是否包含 `kind` + `name/namespace` + `rv`？
- [ ] 如果资源已同步到数据面，access log 中是否有 `sv`？
- [ ] `ctx_map` 中新增的 key 是否可能包含敏感信息？如果是，需要做过滤
- [ ] 插件 stage_logs 中是否记录了完整 username / token？应只记录前缀

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
