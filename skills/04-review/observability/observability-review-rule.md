# 可观测性 Review 规则

> Review 代码变更时，使用本清单确保可观测性设计合理、不引入性能问题或信息爆炸。

## 铁律：数据面零 tracing

数据面热路径（`request_filter`、`upstream_peer`、`response_filter`、`proxy_upstream_filter`、TCP/TLS/UDP 连接处理等）**禁止任何 `tracing::*` 宏**，包括 `debug!`。

**判定方法**：

1. 搜索新增/修改文件中的 `tracing::` 调用
2. 判断调用点是否在请求/连接处理路径上（而非配置加载、启动、ConfHandler）
3. 如果在热路径上 → **必须删除**，改用以下替代方案：

| 场景 | 替代方案 |
|------|---------|
| 请求级结果/决策 | `PluginLog` → access log |
| 连接级错误/事件 | `ctx.err_log` / `ctx.add_error()` |
| 聚合计数 | `global_metrics().xxx()` |

**例外**（仅以下场景允许）：
- ConfHandler / conf_handler_impl 中的配置加载事件（`tracing::info!`）
- 数据面进程级一次性初始化（`once_cell`）
- 致命错误 panic 前（`tracing::error!`）
- access log 发送失败（`pg_logging.rs` 中的 `tracing::warn!`）

详见 [coding-standards/01-log-safety.md](../../03-coding/01-log-safety.md) 铁律 3。

---

## Access Log 合理性

### 新增字段审查

- [ ] 新增字段是否**高频排障必需**？能否从现有字段推断？
- [ ] 字段是**请求级别**的，不是插件内部中间状态？
- [ ] 字段值不会导致**存储爆炸**（无大 body、长 token、无界列表）？

### PluginLog 审查

- [ ] 每条 PluginLog ≤ 40 字节，以 `; ` 结尾
- [ ] 只记录**最终决策**（OK / Deny / Skip / Rate），不记过程
- [ ] 不包含完整 username / token / JWT claims（应 `u=<prefix>***`）
- [ ] 不把应该用 `tracing::debug!` 的信息塞进 PluginLog

### ctx_map 审查

- [ ] `ctx_map` 新增的 key 不包含 JWT / OIDC / JWE claims 等敏感信息
- [ ] 如果必须存入敏感相关 key，是否有 allowlist/blocklist 过滤？

---

## Metrics 合理性

### 不引入爆炸

- [ ] **无 Histogram 类型**（无 `metrics::histogram!`、无 `_bucket` / `_sum` / `_count`）
- [ ] **Label cardinality 有界**（值域 ≤ 数十个）— 绝不使用 path、user_id、ip、trace_id 作为 label
- [ ] 新增指标是否真正必要？现有指标能否已经表达？能计算出来的不要额外存

### 管理方式正确

- [ ] 通过 `GatewayMetrics` struct 统一管理，不直接用 `metrics::counter!()` 宏创建游离指标
- [ ] 命名规范：`edgion_<component>_<what>_<unit>_total / _active`
- [ ] Counter 使用整数 `increment(n: u64)`，不用浮点数

### Gauge 对称性

- [ ] 每个 Gauge `+1` 路径都有对应的 `-1` 路径
- [ ] 优先使用 RAII guard（`Drop` impl）保证对称性
- [ ] 审查异常路径（early return、panic）是否会导致 Gauge 漂移

### 监控价值

- [ ] 新增的 metrics 能回答一个具体的运维/SRE 问题（如"某个 backend 的错误率"、"活跃连接数"）
- [ ] 不是为了"以防万一"而添加——metrics 有长期存储成本

---

## 常见误判

| 现象 | 判定 | 说明 |
|------|------|------|
| ConfHandler 中有 `tracing::info!` | ✅ 合理 | 配置加载不在请求热路径 |
| 插件只写了 `OK; ` 没写详细原因 | ✅ 合理 | 正常通过的插件不需要详细日志 |
| 没有为每个插件单独添加 Counter | ✅ 合理 | 认证/限流结果已体现在 status 和 access log 中 |
| access log 字段只在某些场景有值 | ✅ 合理 | 使用 `skip_serializing_if` 即可 |
