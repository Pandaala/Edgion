---
name: dsl-builtins-path
description: DSL 内建 path/uri 变量参考：req.path / req.uri / req.request_path / req.request_uri / req.query 的视图、行为与使用指引。
---

# DSL 内建：path / uri 变量

> 完整架构上下文：[../../01-architecture/02-gateway/uri-normalization/SKILL.md](../../01-architecture/02-gateway/uri-normalization/SKILL.md)

---

## 变量速查

| 变量 | 视图 | 含义 | 受 rewrite 影响 | alloc 成本 |
|------|------|------|----------------|-----------|
| `req.path` | current（规范化） | 当前 path，post-rewrite | 是 | 1（DSL `Value::Str`） |
| `req.uri` | current（规范化） | 当前 path + 当前 query | 是 | 1（format + Value） |
| `req.query` | current | 当前 query（不规范化） | 是 | 1 |
| `req.request_path` | entry（冻结） | 客户端入口 raw path | 否 | 1 |
| `req.request_uri` | entry（冻结） | 客户端入口 raw path + raw query | 否 | 1（format） |

### 行为变更（迁移提示）

- **`req.path` / `req.uri`** 语义由「raw current」切换为「normalized current」：
  - 对已规范化的请求，值不变
  - 对含 `%XX` 编码字符（非 unreserved 保留字符除外）/ 双斜杠 / dot-segment / path-parameter 的请求，返回规范化后形态
- **`req.request_path` / `req.request_uri`** 是新增变量，反映入口 raw 形态

---

## 使用指引

### 常规路由与 ACL

```dsl
# 正确：用 current 视图做 ACL（覆盖所有编码变体）
if req.path == "/admin" { deny }
if req.path starts_with "/internal/" { deny }
```

### 审计与日志

```dsl
# 记录客户端实际发送的 URI（用于审计）
set_header "X-Entry-Uri" req.request_uri

# 记录 rewrite 后转发到上游的 URI
set_header "X-Upstream-Path" req.path
```

### entry raw deny（谨慎使用）

```dsl
# 场景：需要按字面 raw 拒绝（极少数情况）
# 注意：此类规则对正常客户端可能误拒
if req.request_path contains "%2F" { deny }
```

---

## RegularExpression 重要 Callout

**DSL 中对 `req.path` / `req.uri` 做 regex 匹配时，请求侧已规范化**。以下字面量**永远不会被命中**（静默失效）：

- `%XX` 字面量（如 `%2F`、`%61`）
- `//`、`/./`、`/../`
- `..;`、`;param` 形态

```dsl
# ❌ 错误：以下 regex 对 req.path 永远不匹配（请求侧已规范化）
if req.path matches ".*%2F.*" { deny }      # %2F 已被解码为 /
if req.path matches ".*\/\/.*" { deny }     # // 已被 merge
if req.path matches ".*\/\.\..*" { deny }   # /.. 已被消除

# ✅ 正确：在 normalized space 上写 regex
if req.path matches "^/admin(/.*)?$" { deny }

# ✅ 如需 raw 形态 regex，用 req.request_path
if req.request_path matches ".*%2[fF].*" { deny }
```

---

## 实现位置（内部参考）

- DSL builtin 注册：`src/core/gateway/plugins/http/dsl/lang/compiler/builtins.rs`
- DSL builtin 求值：`src/core/gateway/plugins/http/dsl/lang/builtins.rs`
- session trait：`src/core/gateway/plugins/runtime/traits/session.rs`
- ctx canonical API：`src/types/ctx.rs`
