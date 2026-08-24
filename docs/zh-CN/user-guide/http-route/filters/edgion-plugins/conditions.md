# 插件条件

> `EdgionPlugins` 条目（以及条目级 `dye` 规则）上共享的 `run` / `skip` 谓词。这不是一个独立插件。

插件条件决定某个插件条目（或一条染色规则）是否对当前请求生效。它们共用同一个 `PluginConditions` 对象：

- **`skip`**：或（OR）。任一条件为真即跳过。
- **`run`**：与（AND）。全部条件为真才运行；否则跳过。
- 两者同时配置时，先求值 `skip`。

```yaml
conditions:
  skip:
    - type: keyExist
      key: { type: header, name: X-Internal-Request }
  run:
    - type: keyMatch
      key: { type: header, name: X-Env }
      values: ["production"]
```

## 变体

| `type` | 含义 |
|--------|------|
| `keyExist` | 键存在则为真。 |
| `keyMatch` | 键的值落在 `values` 中，**或**匹配 `regex`，则为真。`negate: true` 是 exclude 的写法。 |
| `timeRange` | 当前时间不早于 `after` 且严格早于 `before`（RFC3339）则为真。任一边界都可省略。 |
| `probability` | 以 `ratio`（0.0–1.0）的概率为真。可选 `key` 使抽样确定化。 |
| `directPeerIpMatch` | 直接 TCP 对端落在 `ips` / `ipRefs` 中则为真。 |
| `remoteIpMatch` | RealIp 解析出的客户端落在 `ips` / `ipRefs` 中则为真。 |
| `secretMatch` | 请求键的值等于指定的 Secret data 值则为真；`secretKey` 指定 data 项。 |
| `hmacAuth` | 请求通过仅校验请求头的 HMAC 验证则为真。HMAC 字段（`secretGroups`、`algorithms` 等）与 `type` 同级。 |
| `jwtClaimMatch` | 已存储的 JWT claim 命中 `values` **或** `regex` 则为真。`claim` 是点分嵌套路径。 |
| `mtlsMatch` | mTLS 客户端证书字段命中则为真。`field` 为 `subject`、`cn` 或 `san`。 |

没有独立的 `include` / `exclude` 类型。原先的 `include` 写成 `keyMatch`；原先的 `exclude` 写成带 `negate: true` 的 `keyMatch`。

`hmacAuth.validateRequestBody` 在条目级 `run` / `skip`、body 门闩和染色条件上会被拒绝。它在 `RequestAccessPolicy` 的 `allOf` 和 `RequestRestriction` 的 `anyOf[].allOf` 中合法。普通的请求值缺失或 HMAC 校验失败以 false 为基值，然后再应用 `negate`。ConfigData 或 Secret 引用未解析是另一种状态：它始终不匹配，`negate` 也不会将其反转。

## ConfigData 与 Secret 引用

`directPeerIpMatch` / `remoteIpMatch` 可以用 `ipRefs` 指向 IpList 类型的 `EdgionConfigData`。`secretMatch` 使用 `secretRef` 与必填的 `secretKey`，且只解析 `Secret.data[secretKey]`，不会遍历 Secret 的全部值。`hmacAuth` 解析 `secretGroups[].secretRefs`。Controller 遍历整棵 Condition 树（条目 `run`/`skip`、body、dye、RequestAccessPolicy `allOf`、RequestRestriction `anyOf`），并在这些 Condition 对象上**就地**写入 `resolvedIps` / `resolvedValues` / `resolvedCredentials`。

- 缺失、未启用、类型错误或未授权的 **IpList** `ipRefs` 目标会报告 `ResolvedRefs=False`，Controller 不会对未解析的 `ipRefs` 编译空 matcher。`secretMatch` 或 HMAC `secretRefs` 引用的 **Secret** 缺失时采用相同报告方式。对于 `secretMatch`，指定的 `secretKey` 不存在、不是 UTF-8 或值为空时也视为未解析。这些错误会进入 `edgion_controller_resource_abnormal` 指标。

```yaml
- type: secretMatch
  key: { type: header, name: X-Origin-Token }
  secretRef: { name: origin-credentials }
  secretKey: token
```
- 条目级或 body 门闩条件存在未解析引用时，EdgionPlugins 运行时会封闭失败，请求收到现有的通用 500 响应。未解析的染色条件只是不匹配。
- 在 `RequestRestriction` 中，条件引用未解析会使 `Deny` 模式返回通用 500 响应。`Allow` 模式会跳过不完整的组并继续检查其他组；若没有组匹配，则执行正常的配置拒绝行为。
- 在 `RequestAccessPolicy` 中，不完整的放行候选不会匹配。系统继续检查同级规则和候选；若没有完整候选放行请求，则策略正常拒绝。
- RequestAccessPolicy 对 ConfigData **内容**另有 partial-good 规则（坏 CIDR 保留有效兄弟；缺失的 IP/URL ConfigData 引用记为 `PartiallyInvalid` 警告）。该 sidecar 规则不适用于条目级或 RequestRestriction 的 `ipRefs`。

## `jwtClaimMatch` 与 `mtlsMatch`

这两个变体不走 `KeyGet`。它们读取一组候选字符串，**任一**候选命中 `values` 或 `regex` 即为真。

**`jwtClaimMatch`** 先在 ctx 的 `jwt_claims` 中按 `claim` 查找，再回落到 `oidc_claims`。前置条件：更早的 `jwt_auth` 或 `openid_connect` 以 `storeClaimsInCtx: true` 运行。前置条件未满足视为缺失值（基值为 false，再应用 `negate`）。

`claim` 为必填字段，且不能是空字符串或仅包含空白字符。所有 Condition
消费者都会执行相同的配置期校验。

```yaml
- type: jwtClaimMatch
  claim: realm_access.roles
  values: ["admin"]
```

**`mtlsMatch`** 读取已暴露的客户端证书。前置条件：会话上有可用的 mTLS 客户端证书。前置条件未满足视为缺失值（基值为 false，再应用 `negate`）。

- `field: subject` / `field: cn` — 0 或 1 个值
- `field: san` — 任一 SAN 条目命中即满足。可选 `sanType`（`DNS`、`URI`、`Email`）按类型过滤并去掉存储前缀（`uri:` / `email:`）。未指定类型时匹配原始存储字符串。

```yaml
- type: mtlsMatch
  field: san
  sanType: URI
  values: ["spiffe://edgion.io/ns/foo"]
```

## 缺失值与 `negate`

每个变体都有 `negate`（布尔，默认 `false`）。

1. 键缺失时，**基值为 false**（匹配本身未成功）。
2. 然后 `negate` 取反。**缺失 + `negate: true` 为 true** — 这就是原先 exclude 对缺失键的判定。

`keyExist` 将不存在视为 false，再应用 `negate`。`timeRange` 与 `probability` 对其计算结果取反。

## `values`（不是 `value`）

精确匹配使用 `values: ["…"]`。单数字段 `value` 已删除。

匹配是**精确匹配**。`values` 不支持通配符（`/api/*` 就是字面量 `/api/*`）。前缀（及其它） glob 形态应写进 `regex`，例如 `/api/*` → `^/api/.*`。

`values` 与 `regex` 是或（OR）：任一侧命中，条件即为真。

`keyMatch`、`jwtClaimMatch` 与 `mtlsMatch` 必须至少配置一个非空的
`values` 或 `regex` 列表。无论条件出现在插件 `run`/`skip`、body、dye、
RequestRestriction 分组还是 RequestAccessPolicy `allOf` 中，都会执行同一校验。
如果写了 `values`，即使同时提供了非空 `regex`，`values` 也不能是空列表。

## `caseSensitive`

布尔，默认 **true**。只作用于 **`values`**。正则的大小写折叠写在模式里（`(?i)`）。

当 `caseSensitive: false` 时，精确匹配使用仅 ASCII 的折叠（`eq_ignore_ascii_case`）。超过 16 个值的列表会编译进 HashSet，折叠规则相同。

## 路径规范化

`keyMatch` 在 `key.type: path` 时，会在**加载时规范化精确 `values`**，使其与请求路径处于同一空间。这对 conditions 是新行为；`RequestRestriction` 对 `Path` 源已经这样做。

`key.type: requestPath` 是入口时的**原始**路径，**从不**规范化。用它匹配客户端实际发送的形态（`/%61dmin` 保持 `/%61dmin`；`path` 看到的是 `/admin`）。

正则**从不**规范化。请按实际观察到的路径形态编写正则。

## 编译后的描述符（访问日志）

每个编译后的条件带有仅运行时存在的描述符，格式为
`<ordinal>:<type>[:<accessor>]`。序号是该条件在对应 `skip` 或 `run` 列表中的
下标（每个列表从 0 起算）。`type` 为 `keyExist`、`keyMatch`、`timeRange`、
`probability`、`secretMatch`、`directPeerIpMatch`、`remoteIpMatch`、`hmacAuth`、
`jwtClaimMatch` 或 `mtlsMatch`。访问器来自 `KeyGet::as_log_str()`（`keyExist` /
`keyMatch` / `secretMatch`，以及配置了 key 的 `probability`）。`jwtClaimMatch`
使用 claim 名（`3:jwtClaimMatch:sub`）；`mtlsMatch` 使用字段名（`3:mtlsMatch:san`）。
IP 与 HMAC 描述符没有访问器。

描述符从不包含匹配值（`values`、`regex`、密钥或 IP 列表）。两个访问器相同的
条件仍靠序号区分：`0:keyMatch:hdr:X-A` 与 `1:keyMatch:hdr:X-A`。

**仅匹配才记录：** 条件匹配时，描述符被推入插件 `log` 数组；未匹配则不写。
body 阶段与染色求值保持静默（不使用 `PluginLog`）。

这是独立于 `cond_skip` 的第二条信号。`cond_skip` 的格式与含义不变，仍回答
“该插件为何被跳过”。

### 从 `log` 数组还原 AND / OR

| 列表 | 逻辑 | 写入内容 | 如何还原 |
|------|------|----------|----------|
| `skip` | 或（OR） | 仅第一条匹配的描述符 | 该 skip 条件命中；后续 skip 未求值 |
| `run` | 与（AND） | 按顺序写入每次成功；第一次失败即停止且**不写入** | 已写入的前缀全部匹配；若插件随后运行，则整个 `run` 列表都匹配。`cond_skip` 为 `!run:…` 时指出第一次失败 |

示例：`run` 列表有三项时，`log: ["0:keyMatch:hdr:X-A", "1:keyMatch:hdr:X-B"]`
表示前两项匹配、第三项未匹配（或无需再评）。同样两条出现在 `skip` 列表中
不可能发生——skip 在第一次匹配后短路。

## 示例

```yaml
conditions:
  run:
    - type: keyMatch
      key: { type: path }
      values: ["/health", "/ready"]
      caseSensitive: true
    - type: keyMatch
      key: { type: path }
      regex: ["^/api/.*"]
    - type: keyMatch
      key: { type: path }
      values: ["/health", "/ready"]
      negate: true   # exclude 写法
    - type: keyExist
      key: { type: header, name: X-Canary }
      negate: true   # 请求头不存在时运行
```

## 相关文档

- [请求限制](./request-restriction.md) — 独立的 allow/deny 插件，使用自己的规则分组
- [请求访问策略](./request-access-policy.md) — 七层策略；`allOf` 使用同一套 `Condition` 枚举
- [染色](./dye.md) — 条目级染色规则复用此 `conditions` 块
