---
name: uri-normalization-review
description: URI 规范化相关 review 判定规则：哪些场景已关闭、哪些仍要报、两条 lint 规则与白名单。
---

# URI 规范化 Review 判定规则

> 对应 spec §9。权威架构文档：[../../01-architecture/02-gateway/uri-normalization/SKILL.md](../../01-architecture/02-gateway/uri-normalization/SKILL.md)

---

## 已关闭场景（不再报）

以下 finding 已被 path normalization 默认机制覆盖，**不再是有效漏洞**，直接关闭：

- 路径类 ACL 用 `RestrictionSource::Path`——path normalization 默认启用（无 `enabled` 开关），此类规则已在规范化 path 上比较，无法被 `%XX` / `//` / `/../` 等绕过
- HTTPRoute / TLSRoute / GRPCRoute 用 `Exact` / `PathPrefix` 路径匹配——配置侧加载时规范化 + 请求侧规范化，匹配在同一 normalized space 上进行
- HTTPRoute `RegularExpression` 匹配——请求侧已规范化（注意：regex 模式本身不规范化，operator 负责把 regex 写在 normalized space 上，见「仍要报场景」第 5 条）
- path-parameter `..;/` 类绕过——`strip_path_parameters` 算法（步骤 A3）已处理
  - **注脚 (v4)**：仅在 `preserve_path_parameters=false`（默认）时关闭。若 operator 启用 opt-out (`preserve_path_parameters=true`)，承诺退化，此类 finding 仍要报，提示 operator 用 `RestrictionSource::RequestPath` regex 拦截 `;` 模式或修正 ACL 规则。
- 标准 percent-encoding 绕过、双斜杠绕过、dot-segment 绕过——均在承诺范围内，已被规范化算法覆盖

---

## 仍要报场景

以下场景**仍要上报**：

1. **DSL 用 `req.request_path` 做 ACL 判断**（而非 `req.path`）——误用 entry raw 视图做安全决策，绕过窗口仍存在。提示改用 `req.path`，或说明为何需要 entry raw（需明确注释）

2. **DSL 安全规则混用 `req.path` 和 `req.request_path` 在同一决策链路且无意图注释**——提示 reviewer 确认操作意图，两视图语义不同，混用可能逻辑错误

3. **上游对 `%2F` 字面量敏感，且业务依赖 raw forward**——Edgion path normalization 不改写 `req_header().uri`，上游收到 raw 入口形态（这是设计承诺，不是漏洞）。需要在文档/注释明示此行为

4. **配置中 `RestrictionSource::Path` 的 `deny` 列表写得像针对 raw**（含 `%XX` 字面量）——提示 operator：`Path` 视图在规范化后比较，`%XX` 字面不会被命中；应改用 `RestrictionSource::RequestPath` 或修正为 normalized 写法

5. **`RegularExpression` 路径配置中含 `%XX` / `//` / `/.` / `..;` 字面量**——请求侧已规范化，含上述字面量的 regex 永远不命中（静默失效）；operator 必须把 regex 改写为 normalized 等价形态

6. **承诺范围外攻击**（overlong UTF-8 / 双重 percent-encoding / 非合规上游解析分歧）——若发现明确攻击场景，提示 operator 启用 `reject_invalid_percent_encoding = true`（扩大承诺范围）。这不是代码 bug，是已知设计边界（见 spec §1.4）

7. **HTTPRoute 注解 `edgion.io/forward-raw-path` 值非 "true" / "false"**——配置非法值，gateway 当作 None 处理但实际 operator 意图丢失；提示修正为合法值

8. **`forward_normalized_path = false` + 业务 ACL 规则同时混用 `Path` 和 `RequestPath`**——全局 raw 模式下 `Path` 视图与 `RequestPath` 视图实际上都是 raw（`Path` 仍走规范化但 upstream 收到 raw），混用可能掩盖 raw forward 副作用

---

## Lint 规则

### Rule 1：禁止业务代码直读 `req_header().uri.path()`

**违规形态**：

```rust
// ❌ 业务代码中直读 pingora 原始字段
let path = session.req_header().uri.path();
let path = ctx.inner.req_header().uri.path();
```

**要求改为**：

```rust
// ✅
let path = session.get_path();          // session trait
let path = ctx.path();                  // ctx canonical
```

**白名单**（以下位置直读合法，不报）：

- `src/types/ctx.rs` 内的 ctx path/uri 方法实现本身
- `pg_early_request_filter.rs` 入口快照代码块（path normalization 入口层）
- ACME / well-known 协议处理代码（`/.well-known/` 路径分发）
- pingora 默认转发逻辑路径
- `session_adapter.rs` 内的 session trait 实现（`get_path` 等的 adapter 实现）

### Rule 2：禁止业务代码裸写 `req_header_mut().set_uri()`

**违规形态**：

```rust
// ❌ 任何业务代码中裸写（除 session_adapter::set_upstream_uri 实现本身）
session.req_header_mut().set_uri(new_uri);
self.inner.req_header_mut().set_uri(parsed);
```

**后果**：ctx 缓存（`normalized_current_path`）与 pingora uri 不一致，导致 ACL / access_log / 路由决策读到 stale path。

**v4 起合法位置 2 处**（且每处都必须紧随 `ctx.uri_finalized = true`）：

1. `src/core/gateway/plugins/runtime/session_adapter.rs::set_upstream_uri`
   - rewrite 类插件触发（url_rewrite / proxy_rewrite / DSL `req.set_uri` / Gateway API URLRewrite filter）
   - 写入次序：`set_uri(parsed)` → `refresh_normalized_path(req, cfg)` → `uri_finalized = true`

2. `src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs::upstream_peer_http` 单点决策块（v4 新增）
   - 入门检查 `!ctx.uri_finalized`
   - 写入次序（仅 `want_raw=false` 分支会 set_uri）：`set_uri(normalized)` → `uri_finalized = true`（无条件置位）

**Rule 2.1** (v4)：上述两处合法位置的 `set_uri` 调用未紧随 `uri_finalized = true` → 强报。

**所有 rewrite 插件**改 path 必须走 trait 方法 `session.set_upstream_uri()`，不能裸调。

---

## 承诺范围摘要

| 攻击类型 | 默认是否覆盖 | 高安全加固方式 |
|----------|-------------|----------------|
| percent-encoding 绕过（`%61dmin`） | ✅ | — |
| `%2F` / `%5C` 编码斜杠绕过 | ✅（decode 后 merge） | `reject_escaped_slashes = true`（直接拒绝含编码斜杠请求） |
| 双斜杠 / dot-segment 绕过 | ✅ | — |
| path-parameter `..;/` 绕过 | ✅ | — |
| overlong UTF-8（`%c0%ae`）| ❌ 承诺范围外 | `reject_invalid_percent_encoding = true` |
| 双重 percent-encoding（`%252E`）| ❌ 承诺范围外 | `reject_invalid_percent_encoding = true` |
| 非合规上游解析分歧 | ❌ 超出控制范围 | operator 自行按上游行为写 ACL 规则 |
