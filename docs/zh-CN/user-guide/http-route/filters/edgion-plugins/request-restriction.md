# Request Restriction 插件

> **🔌 Edgion 扩展**
>
> RequestRestriction 是 `EdgionPlugins` CRD 提供的请求限制插件，不属于标准 Gateway API。

## 概述

`RequestRestriction` 插件通过共享的 `Condition` 枚举限制访问。与专注于 IP 的 `IpRestriction` 不同，本插件匹配请求属性（header、cookie、query、path、method）以及**认证后身份**——mTLS 客户端证书的 subject / SAN / CN（`mtlsMatch`）和已验证的 JWT claim（`jwtClaimMatch`）。内联 IP 集合（`directPeerIpMatch` / `remoteIpMatch` 的 `ips`）可以和 path 条件放在同一分组里。

它仍然是一个**本地 allow/deny 匹配器**，而非完整的 RBAC 编排层：一条条件仍是纯断言，由必填的 `mode` 决定命中意味着放行还是拒绝。

## 模型：有界 ConditionSet + `mode`

必填的 `conditions` 字段使用共享的有界 DNF ConditionSet：

- 顶层 `allOf` 直接包含 Condition，全部匹配时集合才匹配。
- 顶层 `anyOf` 可以包含直接 Condition，或一层 `{ name?, allOf: [...] }` 分组；任一候选匹配时集合即匹配。
- 按配置顺序求值并短路。递归、空集合以及同时包含 `allOf`/`anyOf` 的对象都会被拒绝。
- **`mode` 必填**，决定命中意味着什么：
  - `Deny`：集合匹配 ⇒ **拒绝**；集合不匹配则放行。
  - `Allow`：集合匹配 ⇒ **放行**；集合不匹配则拒绝。

`mode` **没有默认值**。省略它是校验错误，而不是静默回退。

**一条条件是纯断言**，自身不带 allow/deny——极性完全由 `mode` 决定。RequestRestriction 与插件 `conditions.run`/`skip` 使用同一套 ConditionSet 语法，只是外层字段决定的动作不同。

值缺失时，`keyMatch` / 身份匹配为 **false**。在 `mode: Deny` 下，可以用 `keyExist` + `negate: true` 匹配 header 缺失并拒绝请求。

## 功能特性

- **共享 Condition 词表**：与插件 `run`/`skip` 使用同一套 `Condition` 类型
- **请求属性**：header、cookie、query、path、requestPath、method
- **身份匹配**：`mtlsMatch`（subject / CN / SAN，可选 `sanType`）和 `jwtClaimMatch`
- **内联与引用 IP 集合**：`directPeerIpMatch` / `remoteIpMatch` 的 `ips` 与 `ipRefs`（IpList ConfigData）
- **精确匹配与正则匹配**：`values` 精确匹配，`regex` 正则匹配
- **自动性能优化**：精确匹配列表超过 16 个值时自动使用 HashSet (O(1) 查找)
- **布尔组合**：显式的扁平 `allOf`、扁平 `anyOf` 或 OR-of-AND
- **显式极性**：`mode: Deny | Allow`（必填，无默认值）
- **自定义响应**：可配置拒绝时的状态码和消息

## 与 Plugin Conditions 的区别

插件 `run`/`skip` 谓词见 [插件条件](./conditions.md)。两边使用同一套 `Condition` 枚举。

| 维度 | Plugin Conditions | Request Restriction |
|------|------------------|---------------------|
| **定位** | 条件执行框架，决定是否运行某个插件 | 独立插件，直接拒绝不符合规则的请求 |
| **组合** | `run` 与 `skip` 各自包含 ConditionSet | `config.conditions` 包含同一套 ConditionSet |
| **返回行为** | 跳过插件（继续处理链） | 拒绝请求（返回错误响应） |
| **配置位置** | 挂在其他插件上作为条件 | 作为独立插件配置 |
| **使用场景** | 灰度发布、A/B测试、条件执行 | 访问控制、安全防护 |

## 配置参数

### RequestRestrictionConfig

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `mode` | `string`（`Deny` \| `Allow`） | ✅ | - | 极性。`Deny`：任一分组命中即拒绝；`Allow`：任一分组命中即放行，否则拒绝。无默认值，省略即校验错误 |
| `conditions` | `ConditionSet` | ✅ | - | 必须且只能包含一个顶层 `allOf` 或 `anyOf` |
| `status` | `integer` | ❌ | `403` | 拒绝时的 HTTP 状态码 |
| `message` | `string` | ❌ | `Access denied...` | 拒绝时的响应消息 |

### ConditionSet

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `allOf` | `Condition[]` | 二选一 | - | 直接 Condition，按 AND 组合 |
| `anyOf` | `(Condition \| AllOfGroup)[]` | 二选一 | - | 直接 Condition 与一层 AND 分组，按 OR 组合 |

`AllOfGroup` 包含必填且非空的 `allOf`，以及用于归因的可选非空白 `name`。
所有数组都必须非空。每个叶子都是共享的 `Condition`（`type: keyMatch`、
`keyExist`、`mtlsMatch`、`jwtClaimMatch`、`directPeerIpMatch` 等）。
HMAC 的 `validateRequestBody: true` 在此处被接受。

## 已迁移的数据源写法

原先的十一种 restriction 数据源仍然可表达：

| 旧数据源 | 新写法 |
|----------|--------|
| Header | `keyMatch` / `keyExist` + `key: { type: header, name }` |
| Cookie | 同上，`type: cookie` |
| Query | 同上，`type: query` |
| Path | `keyMatch` + `key: { type: path }`（values 仍会规范化） |
| RequestPath | `keyMatch` + `key: { type: requestPath }`（不规范化） |
| Method | `keyMatch` + `key: { type: method }` |
| Referer | `keyMatch` + `key: { type: header, name: Referer }` |
| MtlsSubject | `mtlsMatch` + `field: subject` |
| MtlsCn | `mtlsMatch` + `field: cn` |
| MtlsSan | `mtlsMatch` + `field: san`（可选 `sanType`） |
| JwtClaim | `jwtClaimMatch` + `claim: <path>` |

原先「仅在缺失时命中」的写法，现在是 `keyExist` + `negate: true`。

### 身份数据源

#### mTLS（`mtlsMatch`）

- `field: subject` — 客户端证书的完整 subject **DN 字符串**。
- `field: cn` — 仅 subject **CN**。
- `field: san` — 证书 **SAN** 列表，按「任一条目命中」求值。可选 `sanType`：`DNS` / `URI` / `Email`：
  - **未指定类型**：对原始 SAN 条目做整串匹配（含类型前缀，如 `uri:spiffe://…`、`email:alice@example.com`；DNS 条目是裸值）。
  - **指定类型**：只考虑该类型的 SAN，并在**去掉前缀**的值上匹配（例如 `sanType: URI` 匹配 `spiffe://…`，而不是 `uri:spiffe://…`）。`sanType: DNS` 只匹配无前缀条目，因此排除 `IP:`、`uri:`、`email:` 和 `otherName:`。

> **暴露门控（不对称）。** 客户端证书只有在已暴露给插件层时才可见：
> - **EdgionTls** 路径需要 `edgion.io/expose-client-cert` 注解；否则 mTLS 条件始终缺失（false）。
> - **Gateway `frontendValidation` mTLS** 路径**无需注解**即暴露证书。
>
> 证书未暴露（或连接不是 mTLS）时，所有 mTLS 条件都按缺失（false）处理。

> **优先用 `field: cn` / `field: san`，少用 `subject`。** `subject` 匹配网关特定的 DN 序列化（例如 `CN=foo, O=bar`——短属性名、`, ` 连接、证书条目顺序）。它对顺序和空格敏感，细微格式差异就会导致精确匹配失败。精确身份检查请用 `cn` 或带类型的 SAN；`subject` 留给 `regex`。

#### JWT claim（`jwtClaimMatch`）

- `claim` 是**点分隔的 claim 路径**，例如 `realm_access.roles` 或 `sub`。
- **列表** claim 按「任一元素命中」匹配；标量 string/number/bool 产生一个候选值。
- 值来自上游认证插件写入请求上下文的**已验证** JWT claim：`jwt_auth` 写 `jwt_claims`，`openid_connect` 写 `oidc_claims`。查找顺序是 `jwt_claims` 然后 `oidc_claims`。
- **插件链顺序要求。** 认证插件（`jwt_auth` 或 `openid_connect`）必须在链中**更早执行**且设置 `store_claims_in_ctx: true`；否则上下文中没有 claim，条件为 false。

### 匹配方式

| 配置 | 匹配方式 | 说明 | 示例 |
|------|----------|------|------|
| `values` | 精确匹配 | 完整字符串相等 | `"GET"`、`"/health"` |
| `regex` | 正则匹配 | 正则表达式 | `"(?i).*Bot.*"`、`"^/api/v[0-9]+/.*"` |

> **提示**：正则使用 `(?i)` 前缀做大小写不敏感匹配，例如 `(?i).*bot.*`。

## 执行逻辑

```mermaid
flowchart TD
    A["Request Restriction"] --> B["按配置顺序求值 conditions"]
    B --> C{"ConditionSet 是否匹配?"}
    C --> J{"mode"}
    J -->|"Deny + 匹配"| K["拒绝（status/message）"]
    J -->|"Allow + 不匹配"| K
    J -->|否则| L["继续处理链"]
```

## 使用场景

### 1. 拦截爬虫和机器人

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: block-bots
spec:
  requestPlugins:
    - type: RequestRestriction
      config:
        mode: Deny
        conditions:
          anyOf:
            - type: keyMatch
              key:
                type: header
                name: "User-Agent"
              regex:
                - "(?i).*Bot.*"
                - "(?i).*Spider.*"
                - "(?i).*Crawler.*"
            - type: keyExist
              key:
                type: header
                name: "User-Agent"
              negate: true
        status: 403
        message: "Bot access denied"
```

### 2. 路径白名单（仅 API 路径）

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: api-whitelist
spec:
  requestPlugins:
    - type: RequestRestriction
      config:
        mode: Allow
        conditions:
          anyOf:
            - allOf:
                - type: keyMatch
                  key:
                    type: path
                  values:
                    - "/health"
                    - "/ready"
                  regex:
                    - "^/api/.*"
        status: 404
        message: "Not found"
```

### 3. 只读 API（仅 GET/HEAD）

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: readonly-api
spec:
  requestPlugins:
    - type: RequestRestriction
      config:
        mode: Allow
        conditions:
          allOf:
            - type: keyMatch
              key:
                type: method
              values:
                - "GET"
                - "HEAD"
                - "OPTIONS"
        status: 405
        message: "Method not allowed"
```

### 4. 要求认证 Header

令牌是已知坏值**或** header 缺失时拒绝——两个分组用 OR 组合。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: require-auth
spec:
  requestPlugins:
    - type: RequestRestriction
      config:
        mode: Deny
        conditions:
          anyOf:
            - allOf:
                - type: keyMatch
                  key:
                    type: header
                    name: "X-Auth-Token"
                  values:
                    - "invalid"
                    - "expired"
            - allOf:
                - type: keyExist
                  key:
                    type: header
                    name: "X-Auth-Token"
                  negate: true
        status: 401
        message: "Authentication required"
```

### 5. Referer 限制（防盗链）

只允许内部 Referer——`mode: Allow`，缺失 Referer 视为不允许。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: referer-check
spec:
  requestPlugins:
    - type: RequestRestriction
      config:
        mode: Allow
        conditions:
          anyOf:
            - allOf:
                - type: keyMatch
                  key:
                    type: header
                    name: Referer
                  regex:
                    - ".*example\\.com.*"
                    - ".*example\\.org.*"
        status: 403
        message: "Invalid referer"
```

### 6. 拦截调试参数

通过 query **或** cookie 设置的 debug 标志——两个单条件分组（OR）。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: block-debug
spec:
  requestPlugins:
    - type: RequestRestriction
      config:
        mode: Deny
        conditions:
          anyOf:
            - allOf:
                - type: keyMatch
                  key:
                    type: query
                    name: "debug"
                  values:
                    - "true"
                    - "1"
            - allOf:
                - type: keyMatch
                  key:
                    type: cookie
                    name: "debug"
                  values:
                    - "true"
                    - "1"
        status: 403
        message: "Debug mode not allowed in production"
```

### 7. 分组内 AND：仅拒绝 DELETE /admin

单个分组内两条条件 AND——只有**同时成立**才拒绝。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: protect-admin-writes
spec:
  requestPlugins:
    - type: RequestRestriction
      config:
        mode: Deny
        conditions:
          anyOf:
            - allOf:
                - type: keyMatch
                  key:
                    type: method
                  values: ["DELETE"]
                - type: keyMatch
                  key:
                    type: path
                  regex: [".*/admin/.*"]
        status: 403
        message: "Deleting admin resources is not allowed"
```

### 8. 同一分组内的 IP 与 path

仅对内联对端 IP 集合拒绝 admin 路径。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: local-admin-deny
spec:
  requestPlugins:
    - type: RequestRestriction
      config:
        mode: Deny
        conditions:
          anyOf:
            - allOf:
                - type: directPeerIpMatch
                  ips: ["127.0.0.1", "::1"]
                - type: keyMatch
                  key:
                    type: path
                  regex: [".*/admin/.*"]
        status: 403
        message: "Admin access denied from local peer"
```

### 9. 身份白名单（mTLS SAN 或 JWT claim）

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: identity-allowlist
spec:
  requestPlugins:
    - type: RequestRestriction
      config:
        mode: Allow
        conditions:
          anyOf:
            - type: mtlsMatch
              field: san
              sanType: URI
              values: ["spiffe://edgion.io/ns/payments/sa/checkout"]
            - type: jwtClaimMatch
              claim: "realm_access.roles"
              values: ["admin"]
        status: 403
        message: "Identity not allowed"
```

## 与其他网关的对比

| 能力 | APISIX | Kong | Edgion |
|------|--------|------|--------|
| **User-Agent 限制** | ✅ ua-restriction | ✅ bot-detection | ✅ RequestRestriction |
| **Path/URI 限制** | ✅ uri-blocker | ❌ | ✅ RequestRestriction |
| **Referer 限制** | ✅ referer-restriction | ❌ | ✅ RequestRestriction |
| **Cookie 限制** | ❌ | ❌ | ✅ RequestRestriction |
| **Query 限制** | ❌ | ❌ | ✅ RequestRestriction |
| **Method 限制** | ✅ consumer-restriction | ❌ | ✅ RequestRestriction |
| **统一插件** | ❌（多个独立插件） | ❌（多个独立插件） | ✅（单个插件） |

Edgion 的 `RequestRestriction` 把多种限制能力收进一个插件，配置更简单、维护更容易。

## 注意事项

1. **集合顺序**：Condition 与候选按配置顺序求值，并遵循 `allOf`/`anyOf` 短路。
2. **极性是全局的**：条件是纯断言。命中意味着允许还是拒绝，由 `mode` 一次性决定。
3. **大小写**：精确匹配（`values`）默认大小写敏感；用 `caseSensitive: false` 关闭；正则使用 `(?i)` 前缀。
4. **正则性能**：多个正则模式合并成一条正则（用 `|` 连接），降低匹配开销。
5. **HashSet 优化**：精确匹配列表超过 16 个值时自动使用 HashSet，O(1) 查找。
6. **path 与 requestPath**：`key: { type: path }` 对照规范化后的当前路径（`values` 在加载时规范化）；`key: { type: requestPath }` 是入口时刻的 raw 路径。regex 从不规范化。
7. **缺失值**：`keyMatch` / 身份匹配在值缺失时为 false。用 `keyExist` + `negate: true` 匹配缺失。
8. **ipRefs**：条件可以使用内联 `ips` 和 `ipRefs`（IpList ConfigData）。Controller 在 apply 路径把 `ipRefs` 展开到 `resolvedIps`。缺失、未启用、类型错误或跨命名空间未授权的 IpList 引用 fail-closed（`ResolvedRefs=False`）；网关不会对未解析的操作员 `ipRefs` 编译空 matcher。

## FAQ

### Q1：如何表达黑名单和白名单？

**A**：设置 `mode`。`Deny` 在集合匹配时拒绝；`Allow` 在集合不匹配时拒绝。`mode` 必填，没有可回退的默认值。

### Q2：如何表达 `(A AND B) OR C`？

**A**：使用 `conditions.anyOf`，把 `A` 和 `B` 放进一个嵌套 `allOf`，再把 `C` 作为同级直接 Condition。

### Q3：什么时候需要键名？

**A**：`header`、`cookie`、`query` 需要 `name`。`path`、`requestPath`、`method` 没有 name。`jwtClaimMatch` 使用 `claim`。`mtlsMatch` 使用 `field`。

### Q4：如何在 header 缺失时拒绝？

**A**：把该 header 的 `keyExist` + `negate: true` 放成直接 `anyOf` 候选，或放入一个 `allOf` 分组。在 `mode: Deny` 下，header 缺失会使集合匹配并拒绝请求。

### Q5：如何让精确匹配不区分大小写？

**A**：在条件上设置 `caseSensitive: false`（作用于 `values` 精确匹配）。正则匹配使用 `(?i)` 内联标志，例如 `(?i).*bot.*`。

---

## 获取帮助

如果遇到问题：

1. 查看本文档的 FAQ
2. 查看网关日志
3. 在 GitHub 上开 issue

---

## 相关链接

- [IpRestriction](./ip-restriction.md) — IP 访问控制
- [RateLimit](./rate-limit.md) — 限流
- [插件条件](./conditions.md) — 共享 Condition 词表
