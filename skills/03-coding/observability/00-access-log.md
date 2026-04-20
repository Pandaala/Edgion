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
<结果前缀>[:<关键参数>]; 
```

- 以 `; ` 结尾（方便 grep / 拼接）
- **结果前缀统一约定**：

| 前缀 | 含义 | 写入 API | 示例 |
|------|------|---------|------|
| `OK` | 正常放行 | `plugin_log.push("OK ...; ")` | `OK u=jack; ` |
| `Deny` | 业务级拒绝（认证失败、限流等） | `plugin_log.push("Deny ...; ")` | `Deny no-key; ` |
| `Skip` | 条件不满足，跳过执行 | `plugin_log.push("Skip; ")` 或 `set_cond_skip` | `Skip; ` |
| `Rate` | 限流触发 | `plugin_log.push("Rate ...; ")` | `Rate 429; ` |
| **`err: `** | **异常 / 配置降级** —— 替代数据面 `tracing::warn!`；带空格冒号分隔 | **`plugin_log.push_err("...")`**（见下） | `err: token refresh fail; ` |

- 关键参数用 `k=v` 格式，多个参数空格分隔
- 存量代码里存在无空格旧写法 `"err:failoverTo invalid; "` / `"rl:err:cfg; "`（region_route + rate_limit 共 6 处），**属于 legacy**，由对应 wave 工单逐步迁移到 `push_err`，新代码一律走 `push_err`

### 错误条目 API 约束：必须用 `push_err`

> **权威来源**：本节同时是 `skills/03-coding/01-log-safety.md` 铁律 1 的一部分；此处讲"在 access log 里长什么样"，log-safety.md 讲"为什么这么约束 + 安全规则"。

**API**：

```rust
// 定义：src/core/gateway/plugins/runtime/log.rs
impl PluginLog {
    pub fn push_err(&mut self, kind: &'static str) -> bool;   // 固定 buffer
    pub fn u_push_err(&mut self, kind: &'static str) -> bool; // 调试 buffer
}
```

**调用样子**：

```rust
// ✅ 正确 —— kind 是字面量，不带 plugin 前缀（plugin 身份由 PluginLog.name 承担）
plugin_log.push_err("token refresh fail");    // → "err: token refresh fail; "
plugin_log.push_err("jwks fetch timeout");    // → "err: jwks fetch timeout; "
plugin_log.push_err("cfg invalid");           // → "err: cfg invalid; "

// ✅ bounded enum 错误 —— 手动映射成静态标签
match jwt_err.kind() {
    ErrorKind::InvalidSignature => plugin_log.push_err("jwt sig invalid"),
    ErrorKind::ExpiredSignature => plugin_log.push_err("jwt expired"),
    _                           => plugin_log.push_err("jwt other"),
};

// ❌ 编译不过 —— &'static str 签名拦住所有动态字符串
plugin_log.push_err(&format!("oidc: {}", e));
plugin_log.push_err(&err.to_string());
plugin_log.push_err(dynamic_kind.as_str());

// ❌ 即使能编译，也禁止走 push 绕过 push_err 写错误（泄漏风险）
plugin_log.push(&format!("Config error: {}; ", err));       // Display 可能含 token / claim
plugin_log.push(&format!("Auth failed for user {}", name)); // PII 入 PluginLog
```

**`kind` 字面量约定**：

- 小写单词 + **空格**分隔（不用下划线），15-40 字节
- 只写"发生了什么"（事件名），**不带 plugin 前缀**（`PluginLog.name` 已经承担）
- 不含动态值（token / claim / URL / header / 错误 Display）
- 允许 `<event>:<action>` 两段冒号语法（两段都是完整词），详见下文「PluginLog 条目措辞规范」§1 rule 4

**何时用 `push_err` vs 继续写到 `ctx.err_log`**：`push_err` 做**分类**；`ctx.err_log` 留给**该错误必须保留的脱敏详情**（如后端地址）。同一错误只选一处记录，不要双写。

**安全背景**：完整的 `&'static str` 约束理由、`Box::leak` 逃逸注意点、安全清单 → `skills/03-coding/01-log-safety.md` 的「PluginLog 错误条目：必须用 `push_err`」小节。

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

// OIDC — token refresh 失败（错误分类，替代 tracing::warn!）
log.push_err("token refresh fail");

// RegionRoute — failoverTo 配置无效（插件降级，该 region 跳过 redirect）
//   新代码：
log.push_err("failover to invalid");
//   旧代码 legacy 写法（`region_route/plugin.rs` 尚未迁移）：
//   log.push("err:failoverTo invalid; ");
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

// ❌ 错误路径走 push + format! —— 泄漏风险 + 绕过 push_err 约束
log.push(&format!("OIDC token refresh failed: {}", jwt_err));
log.push(&format!("Auth failed for user {}", username));
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

---

## PluginLog 条目措辞规范

### 1. 条目字面量风格（push 与 push_err 共用）

1. 全小写英文单词 + 空格分词；每条 15-40 字节，≤ 4 个词（含动作词）
2. 禁缩写。白名单（不展开）：ip, tls, http, https, tcp, udp, sni, id, url,
   dns, jwt, jwks, cors, csrf
3. 允许数字字面量：rate 429, retry 2, size 1024
4. 事件+动作用冒号连接（两段都完整词，无空格粘连）：
   `"key miss:allow; "`、`"jwks refresh:stale; "`、`"upstream select:failover; "`
   单段信息不加冒号：
   `"upstream b selected; "`、`"OK user jack; "`
   （尾部 "; " 见 rule 8）
5. 保留前缀动作词（不可改名）：OK / Deny / Rate / Allow / Skip
6. **敏感动态值禁止**。push / push_err 的字符串中不得出现：token、claim 值、
   raw header value、cookie、email、secrets（API key、password、session id）、
   request/response body。
   **允许的非敏感动态值**（仅 push 可用；push_err 参数已是 &'static str 编译期强制）：
   - `ip`, `port`, `path`, `http status code`, `method`
   - 数值（count, duration_ms, size, retry_n）
   - user 身份（username, user_id）—— access log 已携带，一致性优先
   - 闭集 tag（region name, upstream group name，代码层面的静态标签）
   **错误路径永远走 push_err**（&'static str），绝不用 `push(&format!("...{}", err))`
   拼错误信息 —— Display 可能泄漏 token/claim。
7. 插件名不写入条目（已在 PluginLog.name 字段）。修正示例：
   "rl:nok:a" → "key miss:allow"
8. **末尾 "; " 分隔**：push 条目手动加 "; " 结尾，与 push_err 自动添加的 "; " 保持一致。
   示例：`log.push("OK user jack; ")`、`log.push(&format!("Direct {}:{}; ", ip, port))`。

### 2. push vs push_err 判别

- push：正常决策、分支命中、业务语义的 allow/deny、限流结果、降级但请求继续
  例：`"OK user jack; "` / `"Deny bad credential; "` / `"Rate 429; "` / `"key miss:allow; "`
- push_err：per-request 异常（config 读不出、远程调用失败、解码失败、内部
  unreachable 分支）
  例：`"token refresh fail"` / `"cache load fail"` / `"jwks fetch:timeout"`
  （push_err 的 kind **不带** "; "，运行时自动补 `"err: <kind>; "`）
kind 形如 `"<action> fail"` 或 `"<action>:<reason>"`。同一插件内两者可各自多次调用，
最终 log 序列化为数组：
`log: ["OK user jack; ", "err: jwks fetch:timeout; ", "OK fallback; "]`。

### 3. 关键节点筛选

应加（删掉该条后，读 access log 看不出请求走向）：
- 选了哪个 upstream / endpoint / region / failover
- 选了哪条 auth 算法或 key 来源
- 限流命中哪个 key/bucket、结果是 allow/throttle/deny
- 缓存命中/未命中、是否走降级
- 匹配到了哪条 rule/header/pattern
- 所有能进入 PluginRunningResult::ErrTerminateRequest 的分支（必须 push_err 留痕）

不加：
- 进入/离开函数、header 拆分、config 读取
- 循环内部每次迭代
- wrapper 层 run_if/skip_if（已由 conditional_filter.rs 自动 set_cond_skip）
- 异常已有 push_err 就别再加一条 push("failed") 复述

### 4. tracing 边界（硬规则）

per-request 调用链（filter 入口同步调用栈）范围内：
- 错误 → push_err
- 决策 → push
- 禁止 tracing::{error,warn,info,debug,trace}!

允许保留 tracing::*! 的位置：插件构造函数 / new() / config load / 后台任务
（JWKS 轮换、cache refresh timer、Redis reconnect）/ 模块初始化。
