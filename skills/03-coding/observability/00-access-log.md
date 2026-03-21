# Access Log 设计与规范

> Access Log 的核心原则：**单条日志能还原整个请求**。
> 包含 AccessLogEntry 字段设计、PluginLog 格式规范、常见场景速查。

## 核心原则

### 单条日志能还原整个请求

每一条 access log 应当包含足够的信息，让运维人员无需翻其他日志就能定位问题。关注点包括：

- **路由命中情况**（哪条 route / backend 被选中）
- **插件执行关键结果**（认证通过/拒绝、限流触发、改写了什么）
- **上游连接情况**（重试次数、latency 各阶段、最终状态码）
- **请求身份信息**（real IP、trace-id、sni）

### 只记录关键节点，不记过程

❌ **不要记录**：
- 插件内部的中间状态（"starting auth check"、"token parsed"）
- 每次 header 读取、每次条件判断
- 大量重复的 debug 信息（这些应进 `tracing::debug!()`）

✅ **要记录**：
- 插件的**最终决策**（允许 / 拒绝 / 改写了什么值）
- **异常路径**（连接失败、超时、校验错误的原因摘要）
- **关键参数**（命中的 key、选中的 backend、retry 次数）

---

## AccessLogEntry 字段

`src/core/gateway/observe/access_log/entry.rs` — 由 `AccessLogEntry::from_context(ctx)` 在 `pg_logging.rs` 的 `logging()` 阶段生成：

| 字段 | 含义 |
|------|------|
| `client-addr` | TCP 直连 IP |
| `remote-addr` | 真实客户端 IP（经 RealIp 插件处理后） |
| `host` / `path` | 请求 host 和 path |
| `x-trace-id` | trace id（客户端传入或自动生成 UUID） |
| `status` | 最终响应状态码 |
| `sni` | TLS SNI（HTTPS 连接） |
| `tls_id` | TLS 连接 ID（用于关联 tls_access.log） |
| `discover_protocol` | 自动探测协议（grpc / websocket 等） |
| `match_info.sv` | sync_version — gRPC 同步版本号，用于关联控制面日志（0 时省略）。详见 [../00-logging-and-tracing-ids.md](../00-logging-and-tracing-ids.md) |
| stage_logs | 各阶段插件日志（见下方 PluginLog） |
| upstreams | 上游连接详情（ip、port、ct/ht/bt/et、retry） |

### 何时增加新字段

**满足以下条件才增加：**
1. 该字段在排障时**高频需要**，且无法从其他现有字段推断
2. 字段是**请求级别**的（不是插件内部的中间状态）
3. 字段值**不会导致存储爆炸**（避免把大 body 或长 token 放进去）

**正确示例**：
- 新增 `grpc_service` / `grpc_method`（gRPC 路由排障必要信息）✅
- 新增 `client_cert_info`（mTLS 开启后需要知道是哪个证书）✅

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
// KeyAuth — 认证通过
log.push("OK u=jack; ");

// KeyAuth — key 不存在
log.push("Deny no-key; ");

// RateLimit — 触发限流
log.push(&format!("Rate 429 k=user:{}; ", user_id));

// RateLimit — 未触发
log.push("OK; ");

// BasicAuth — 认证失败
log.push("Deny bad-cred; ");

// Redirect — 执行了跳转
log.push(&format!("-> {} {}; ", code, location));

// DirectEndpoint — 强制指定了 endpoint
log.push(&format!("Direct {}:{}; ", ip, port));
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

### Buffer 限制

`PluginLog` 有固定大小的 buffer（默认 100 字节）。每条日志应保持在 **20~40 字节**以内，留出多个插件同时写入的空间。超出 buffer 的内容会被截断。

详细调试信息请用 `tracing::debug!`。

---

## 常见场景速查

### 场景 1：插件做了认证决策
```rust
log.push("OK u=alice; ");          // 通过
log.push("Deny missing-token; ");  // 拒绝，附原因
```
不需要加 metrics（认证结果已体现在 `requests_failed` 和 `backend_requests_total` 的 status 中）。

### 场景 2：插件改写了请求
```rust
log.push(&format!("Rewrite -> {}; ", new_path));
```

### 场景 4：上游连接失败
上游的 error 信息通过 `UpstreamInfo.err` 字段进入 access log，不需要在插件里单独记录。

### 场景 5：tracing 日志还是 access log？

| 信息类型 | 去处 |
|---------|------|
| 请求最终决策 | `PluginLog` → access log |
| 排查中间步骤 | `tracing::debug!()` → 系统日志 |
| 异常/告警 | `tracing::warn!()` / `tracing::error!()` → 系统日志 |
| 性能数据 | `Gauge` / `Counter` in `metrics.rs` → Prometheus |

---

## Key Files

- `src/types/ctx.rs` — `EdgionHttpContext`、`MatchInfo`（含 `sv` 字段）、`MatchedInfo`（含 `sv` 字段）
- `src/core/gateway/observe/access_log/entry.rs` — `AccessLogEntry`
- `src/core/gateway/observe/access_log/logger.rs` — `AccessLogger`
- `src/core/gateway/observe/logs/logger_factory.rs` — `create_async_logger()`
- `src/core/gateway/plugins/runtime/log.rs` — `PluginLog`, `LogBuffer` (100-byte SmallVec)
