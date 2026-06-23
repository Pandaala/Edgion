# Path Normalization（路径规范化）

Edgion 在每个请求入口强制执行路径规范化，消除路径类访问控制规则可被 percent-encoding / 双斜杠 / dot-segment / path-parameter 绕过的缺口。**此功能始终启用，没有全局开关。**

---

## 为什么需要规范化？

典型攻击示例：

- 配置 `RequestRestriction` 插件拦截 `/admin`（`Path` 规则 `values: ["/admin"]`）
- 攻击者请求 `/%61dmin`（`/admin` 的 percent-encoding 形态）
- 未规范化时：字符串精确比较 `/%61dmin ≠ /admin` → 绕过成功
- 规范化后：`/%61dmin` → `/admin` → 命中拦截规则 → 拒绝

---

## 双视图概念

Edgion 提供两个路径视图：

| 视图 | 变量 | 含义 |
|------|------|------|
| **current（默认）** | `$path` / `$uri` / `req.path` / `req.uri` | 规范化后当前 path，受 rewrite 插件影响 |
| **entry（入口）** | `$request_path` / `$request_uri` / `req.request_path` / `req.request_uri` | 客户端实际发送的原始 path，冻结不变 |

**核心原则**：

- 路由匹配、ACL 决策、access_log 默认使用 **current 视图**（已规范化）
- 审计、安全调查需要客户端原始形态时使用 **entry 视图**

---

## 配置选项

`EdgionGatewayConfig` CRD 提供两个可选加固开关：

```yaml
spec:
  pathNormalization:
    rejectEscapedSlashes: false           # 含 %2F/%5C 的请求直接 400 拒绝
    rejectInvalidPercentEncoding: false   # 含非法 percent-encoding 的请求直接 400 拒绝
```

**注意**：没有 `enabled` 字段——路径规范化是 Edgion 的必要行为，不可禁用。

| 选项 | 默认 | 适用场景 |
|------|------|----------|
| `rejectEscapedSlashes` | `false` | 高安全场景，要求 path 中不允许字面编码斜杠 |
| `rejectInvalidPercentEncoding` | `false` | 高安全场景，需要拦截 overlong UTF-8 等异常输入 |

### Access Log 中的原始 URI 输出

默认情况下 access log 仅输出规范化后的 `path` 字段，不输出任何 raw 形态字段、不输出 query。
若需要审计客户端原始完整 URI，开启 Gateway 侧 YAML 选项：

```yaml
access_log_options:
  include_raw_uri: true
```

启用后 access log JSON 多一个字段 `request_uri`，值为客户端入口完整 raw URI（raw path + raw query）。详见 [Access Log](ops-guide/observability/access-log.md)。

---

## 安全承诺范围

**Edgion 保证在以下情况下 ACL 不可绕过**：

- `%XX` 形态的 unreserved 字符编码绕过（如 `/%61dmin`）
- `%2F` / `%5C` 编码斜杠绕过
- 双斜杠绕过（`/admin//x`）
- dot-segment 绕过（`/admin/../secret`）
- path-parameter 绕过（`/admin/..;/secret`，CVE-2022-35202 同类）

**以下情况不在默认保证范围内**：

- overlong UTF-8 编码（`%c0%ae%c0%ae`）——启用 `rejectInvalidPercentEncoding` 可加固
- 双重 percent-encoding（`%252E%252E`）——同上
- 上游框架对 raw URI 的特殊解析行为（超出 Edgion 控制范围）

---

## 迁移指引（升级到 path normalization 版本）

### 1. `req.uri` / `$uri` 语义变更

**旧行为**：返回 raw current URI（含客户端的原始编码形态）

**新行为**：返回 normalized current URI（规范化后）

若审计日志依赖原始 URI：

```yaml
# 旧写法（可能拿到 raw）
- template: "uri=${uri}"

# 新写法（明确指定 entry raw）
- template: "entry_uri=${requestUri}"
```

### 2. `RegularExpression` 路径匹配

**现有 regex 中含以下字面量将静默失效**（请求侧已规范化，永不命中）：

- `%XX` 字面量（如 `%2F`、`%61`）
- `//`（双斜杠）
- `/./`、`/../`（dot-segment）
- `..;`（path-parameter 绕过形态）

必须将 regex 改写为在 normalized space 上有效的形态：

```yaml
# ❌ 会静默失效——请求 /%61dmin 已规范化为 /admin，这个 pattern 永远不匹配
path:
  type: RegularExpression
  value: "/%61dmin"

# ✅ 正确写法
path:
  type: RegularExpression
  value: "/admin"
```

### 3. `RestrictionSource::Path` 配置

`Path` source 规则的 `values` 加载时会被规范化（与请求侧匹配同一空间）。若有 `/admin/` 之类 trailing slash，会被规范化为 `/admin`——INFO 日志会记录此转换，不静默。

若需要按字面 raw 拒绝，改用 `RestrictionSource::RequestPath`。

### 4. `;param` 对白名单规则的副作用

path normalization 会剥除每个 segment 中的 `;param=...`（防 `..;/` 类绕过）。

- 影响 **黑名单分组**（`mode: Blacklist`）：`/admin/..;jsessionid=x/secret` 在 ACL 视图为 `/secret`，能正确命中 `Path` 规则的 `values: ["/secret"]` ✓
- 影响 **白名单分组**（`mode: Whitelist`）：请求 `/api/users;v=1/data` 在 ACL 视图为 `/api/users/data`——若配置 `Path` 规则 `values: ["/api/users"]` exact，**不命中**

上游仍收到含 `;param` 的 raw path，依赖 `;jsessionid` 的应用不受影响。

---

## 常见使用场景

### 防止管理路径访问（推荐写法）

```yaml
- type: RequestRestriction
  config:
    mode: Blacklist
    ruleGroups:
      - rules:
          - name: block-admin
            source: Path             # current 视图，覆盖所有编码变体
            values:
              - "/admin"
              - "/internal"
            onMissing: NoMatch
```

### 审计日志记录客户端原始 URI

```yaml
- type: CtxSet
  config:
    rules:
      - name: entry_uri
        template: "${requestUri}"  # 客户端入口 raw URI，冻结
```

---

## 相关文档

- [EdgionGatewayConfig](ops-guide/configuration/edgion-gateway-config.md)——`spec.pathNormalization` 配置参考
- RequestRestriction 插件：[user-guide/http-route/filters/edgion-plugins/request-restriction.md](user-guide/http-route/filters/edgion-plugins/request-restriction.md)

---

## Upstream 收到什么 path（v4）

Edgion v4 起默认把规范化后的 path 转发到上游，与 Envoy / Kong 行为对齐。决策按以下优先级：

| 优先级 | 通道 | 来源 | 默认 |
|---|---|---|---|
| 0 | rewrite 类插件已写过 path | `url_rewrite` / `proxy_rewrite` / DSL `req.set_uri` 等 | 用 rewrite 后的 path（终态） |
| 1 | per-route 注解 `edgion.io/forward-raw-path` | HTTPRoute `metadata.annotations` | `"true"` 强制 raw / `"false"` 强制 normalized |
| 2 | 网关互转（region_route） | `region_route` 插件 | raw（避免与对端二次规范化冲突） |
| 3 | 全局开关 `forward_normalized_path` | `path_normalization` 配置段 | `true`（normalized）|
| 默认 | 兜底 | — | normalized |

### 与 Envoy / Kong 对比

| Gateway | 默认 upstream 视图 | 是否可关 |
|---|---|---|
| Envoy `normalize_path` | normalized | 是 |
| Kong Traditional router | normalized | 是 |
| **Edgion v4** | **normalized**（v4 与业界对齐）| 是（`forward_normalized_path: false`）|
| Edgion v3.1 及更早 | raw | — |

### 升级注意

如果你的上游服务对 path 字面敏感（例如对 `%2F` 字面有特定路由逻辑），升级到 v4 后默认会收到 decode 后的形态。两种解决：

1. **per-route opt-out**：在受影响的 HTTPRoute 加 `edgion.io/forward-raw-path: "true"` 注解
2. **全局回退**：在 `path_normalization` 设 `forward_normalized_path: false`（影响所有路由）

### `;param` 兼容场景

如果你的上游应用依赖 path-parameter（如 `;jsessionid=xxx`），可启用：

```yaml
path_normalization:
  preserve_path_parameters: true
```

注意：开启后 `..;/` 类绕过攻击不再被默认拦截（参见 spec §14.4）。建议同时用 `RestrictionSource::RequestPath` regex 拦截 `;` 字符的可疑模式。
