# 控制面日志与 Tracing 规范

> 控制面使用 `tracing` crate 做结构化日志。本文档覆盖结构化日志规范、Level 选择、
> 热路径约束、敏感信息处理、错误上下文、instrument 命名等最佳实践。

## 信息分层：三层日志各司其职

| 层次 | 工具 | 目标受众 | 保留时间 |
|------|------|---------|---------|
| 请求粒度 | access log（JSON） | 运维排障、审计 | 天~周 |
| 系统事件 | `tracing::info/warn/error` | 开发调试、告警 | 小时~天 |
| 性能聚合 | Prometheus metrics | SRE、Grafana | 月 |

**原则**：access log 聚焦请求维度；系统日志聚焦组件事件；metrics 聚焦趋势聚合。三者互补，不要用 access log 当系统日志用，也不要用 metrics 存原始请求细节。

---

## 结构化日志规范

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
tracing::info!(component = "grpc_server", ...);
tracing::info!(component = "k8s_status", ...);
tracing::info!(component = "conf_client", ...);
```

---

## Level 选择

| Level | 适用场景 |
|-------|---------|
| `error!` | 影响请求/数据的错误（需要立即处理） |
| `warn!` | 可恢复的异常、重试、降级（需要关注但不紧急） |
| `info!` | 重要的生命周期事件（启动、关闭、reload、连接建立/断开） |
| `debug!` | 排查时有用的中间状态（正常运行时不需要看） |
| `trace!` | 极度详细的流程追踪（性能分析专用） |

---

## 热路径约束

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

---

## 敏感信息不进日志

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

---

## 错误信息要有定位价值

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

---

## 优雅降级

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

---

## instrument 命名

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

## 相关规范

- **[05-coding-standards/00-logging-and-tracing-ids.md](../05-coding-standards/00-logging-and-tracing-ids.md)** — rv / sv 传播机制和排障流程
- **[05-coding-standards/01-log-safety.md](../05-coding-standards/01-log-safety.md)** — 敏感信息防泄漏、数据面禁止 tracing 的铁律
