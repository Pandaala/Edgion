---
name: uri-normalization
description: URI 规范化架构权威文档：全栈命名表、双视图概念、铁律、安全承诺范围、set_upstream_uri 集成点。
---

# URI 规范化（Path Normalization）

> 权威来源：`docs/superpowers/specs/2026-04-26-uri-normalization-design.md`
> 相关 review 规则：[../../04-review/uri-normalization/SKILL.md](../../../04-review/uri-normalization/SKILL.md)

Edgion 在每个请求入口强制执行路径规范化（不可禁用），消除路径类 ACL 可被 percent-encoding / 双斜杠 / dot-segment / path-parameter 绕过的缺口。

---

## 概念：双视图架构

| 视图 | 反映 | 受 rewrite 插件影响 | 典型用途 |
|------|------|---------------------|----------|
| **current 视图**（默认名） | post-rewrite 当前 path/uri，规范化后 | 是（adapter 在 `set_upstream_uri` 内自动 refresh） | 路由匹配、ACL 决策、access_log 默认 |
| **entry 视图**（`request_` 前缀） | 客户端入口时 raw 形态，**冻结** | 否 | 审计、日志、特殊场景 raw deny |

**概念分层**：

```
operator 视角:    $path / $uri / $query           （current 视图）
                  $request_path / $request_uri    （entry 视图）
                  req.path / req.uri / ...        （DSL 同样区分）
                       ↑ 模板/DSL 表面，全部委托到 session trait

session trait:    get_path() -> &str              get_request_path() -> &str
                  get_uri() -> String             get_request_uri() -> String
                  get_query() -> Option<String>
                       ↑ 插件接口

EdgionHttpContext canonical API:
                  ctx.path()                     （current 视图，读缓存）
                  ctx.request_path()             （entry 视图，frozen）
                  ctx.query(req)                 （current 视图，passthrough）
                  ctx.uri(req)                   （current path + current query）
                  ctx.request_uri()              （entry path + entry query）
                  ctx.refresh_normalized_path(req)（refresh current cache，
                                                    由 session_adapter::set_upstream_uri 内部调）
                       ↑ 单一事实源

pingora 字段:     req_header().uri
                       ↑ 业务代码禁止直读（白名单见下方「铁律与白名单」）
```

---

## 全栈命名表（§4 权威副本）

| 层级 | current 视图 | entry 视图（冻结）|
|------|--------------|-------------------|
| **EdgionHttpContext 方法**（canonical） | `ctx.path() -> &str` | `ctx.request_path() -> &str` |
| | `ctx.uri(req) -> String` | `ctx.request_uri() -> String` |
| | `ctx.query(req) -> Option<&str>` | （单一，passthrough） |
| **EdgionHttpContext 写方法**（仅 adapter 内部用） | `ctx.refresh_normalized_path(req)` | — |
| **pingora 原始字段**（业务禁止直读，白名单见下） | — | `req_header().uri` |
| **session trait** | `get_path() -> &str` | `get_request_path() -> &str` |
| | `get_uri() -> String` | `get_request_uri() -> String` |
| | `get_query() -> Option<String>`（保留兼容） | （单一） |
| **DSL builtin** | `req.path` | `req.request_path` |
| | `req.uri` | `req.request_uri` |
| | `req.query` | （单一） |
| **ctx_set 模板变量** | `$path` | `$request_path` |
| | `$uri` | `$request_uri` |
| | `$query` | （单一） |
| **`request_restriction` source** | `Path` | `RequestPath` |

### 命名规则

- 默认名（`path` / `uri`）= current normalized 视图（缓存，反映 rewrite）
- `request_` 前缀 = entry raw 视图（冻结，不受 rewrite 影响）
- query 不规范化，单一名（`query`）

### query 视图不对称（已知 audit gap）

`$request_query` / `req.request_query` 不存在。entry query 仅通过 `$request_uri` 间接获取——从中 split 即可。加该变量的 ROI 低（大多数审计关心完整 URI），YAGNI。

---

## 铁律与白名单

**铁律**：业务代码**禁止直读** `req_header().uri.path()`，必须使用 `ctx.path()` 或 `session.get_path()`。

白名单（以下位置直读合法）：

- `src/types/ctx.rs` 内的 ctx path/uri 方法实现本身
- `pg_early_request_filter.rs` 入口快照代码块（path normalization 入口层）
- ACME / well-known 协议处理代码
- pingora 默认转发逻辑路径
- `session_adapter.rs` 内的 session trait 实现

---

## `set_upstream_uri` 集成点

`session_adapter.rs:443` 是整个 codebase 中**唯一**写 `req_header_mut().set_uri()` 的位置。所有 rewrite 插件（`proxy_rewrite`、`url_rewrite`、`request_redirect`、DSL `req.set_uri` 等）全部走 trait 封装：

```rust
// session_adapter.rs:443（修改后）
fn set_upstream_uri(&mut self, uri: &str) -> PluginSessionResult<()> {
    let parsed_uri = uri.parse::<Uri>().map_err(|e| Box::new(e) as PluginSessionError)?;
    self.inner.req_header_mut().set_uri(parsed_uri);
    self.ctx.refresh_normalized_path(self.inner.req_header());  // 新增此行
    Ok(())
}
```

**插件层零改造**：所有现有 rewrite 插件保持原状，trait 调用自动获得 ctx 缓存同步。

rewrite 后效果：

- `ctx.path()` / `$uri` / `req.uri` → 反映**新 path**（缓存已 refresh）
- `ctx.request_path()` / `$request_uri` → 仍反映**入口 raw**（entry 视图冻结）
- 上游收到 rewrite 后的 path

---

## 安全承诺范围（§1.4 摘要）

ACL 防绕过**在以下情况成立**（承诺范围内）：

- `%XX` 形态的 unreserved 字符 / `%2F` / `%5C` 编码绕过
- 双斜杠绕过（`/admin//x` 类）
- 标准 dot-segment 绕过（`/admin/../secret`）
- path-parameter 绕过（`/admin/..;/secret` 类，CVE-2022-30065 / 35202 同类）
- 编码 dot-segment 组合（`/admin/%2E%2E/secret`）

**承诺范围外**（不保证）：

- overlong UTF-8 编码穿越（`%c0%ae%c0%ae` IIS-style）——默认保留 encoded 形态不展开；高安全场景启用 `reject_invalid_percent_encoding`
- 双重 percent-encoding（`%252E%252E`）——默认仅做一层 decode；高安全场景同上
- 非 RFC 3986 合规上游对 raw URI 做特殊解析

高安全场景推荐启用：

```toml
[path_normalization]
reject_escaped_slashes = true         # 拦截含 %2F / %5C 请求
reject_invalid_percent_encoding = true # 拦截含非 UTF-8 / 非法 hex percent-encoding 请求
```

完整安全承诺范围见 spec §1.4。

---

## 规范化算法（Edgion BASE，固定，不可单独开关）

步骤顺序固定为：

1. **strip_path_parameters**：剥除每个 path segment 中的 `;param=...`（防 `..;/` 类绕过）
2. **percent-decode**（按白名单解码）
3. **merge_slashes**（连续 `/` 合并为单个）
4. **remove_dot_segments**（RFC 3986 §5.2.4 算法）

注意：path normalization **不改写** `req_header().uri`——上游 / ForwardAuth / mirror 收到 raw 入口形态。仅 rewrite 插件合法地改写 uri（走 trait `set_upstream_uri`）。

---

## 何时用哪个视图（插件作者决策指南）

| 场景 | 推荐视图 | 方法 |
|------|----------|------|
| 路由匹配、ACL 访问控制 | current | `session.get_path()` / `req.path` |
| access_log 默认记录请求 path | current | `$path` / `$uri` |
| 审计：客户端实际发送了什么 | entry | `session.get_request_uri()` / `$request_uri` |
| deny 规则要防编码绕过 | current | `RestrictionSource::Path`（自动规范化） |
| deny 规则要按字面 raw 拒绝 | entry | `RestrictionSource::RequestPath` |
| rewrite 后记录新 path | current（已 refresh） | `$uri` |
| rewrite 后仍需原始 path | entry | `$request_uri` |
| 改写请求 path | — | `session.set_upstream_uri(&new_uri)` |
| 默认 upstream 转发 path 视图 | current normalized | 自动（v4 spec §14）|
| 路由级 force raw 转发 | entry view | annotation `edgion.io/forward-raw-path: "true"` |
| 路由级 force normalized 转发（盖 region_route）| current normalized | annotation `edgion.io/forward-raw-path: "false"` |

**性能提示**：热路径上优先用 `path` / `request_path`（`&str`，零额外 alloc）而非 `uri` / `request_uri`（format alloc）。

---

## Upstream Path Forwarding 决策（v4）

`pg_upstream_peer::upstream_peer_http` 入口单点决策，5 通道优先级：

| 通道 | 触发 | 效果 |
|---|---|---|
| 0（隐含最高）| `ctx.uri_finalized` 已 true | 跳过决策（rewrite 终态 / 已决策） |
| 1 | 注解 `edgion.io/forward-raw-path: "true"` | 强制 raw |
| 1 | 注解 `edgion.io/forward-raw-path: "false"` | 强制 normalized（盖过 region_route） |
| 2 | `external_jump.preserve_raw_path = true`（region_route 类）| raw |
| 3 | 全局 `forward_normalized_path = false` | raw |
| 默认 | 以上都不命中 | normalized |

`uri_finalized` 单 flag 覆盖 rewrite 终态 + 单点决策已落定 + 重试 / mirror / fan-out 一致性。

合法 `req_header_mut().set_uri()` 写入仅 2 处（lint Rule 2）：
- `session_adapter::set_upstream_uri`（rewrite 类）
- `pg_upstream_peer::upstream_peer_http` 单点决策块（v4）
两处都必须紧随 `ctx.uri_finalized = true`。

详情见 spec `docs/superpowers/specs/2026-04-26-uri-normalization-design.md` §14。
