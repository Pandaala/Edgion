# 插件条件

> `EdgionPlugins` 条目（以及条目级 `dye` 规则）上共享的 `run` / `skip` 谓词。这不是一个独立插件。

插件条件决定某个插件条目、body 门闩或染色规则是否对当前请求生效。
`skip` 与 `run` 是动作字段，每个字段都包含一个显式的 `ConditionSet`。
两者同时配置时先求值 `skip`；`skip` 集合匹配时跳过动作，`run` 集合不匹配时也跳过动作。

```yaml
conditions:
  skip:
    anyOf:
      - type: keyExist
        key: { type: header, name: X-Internal-Request }
  run:
    allOf:
      - type: keyMatch
        key: { type: header, name: X-Env }
        values: ["production"]
```

## ConditionSet 语法

`EdgionPlugins` 不限制整个资源中的 IP 匹配条件总数，包括通过 ConfigData 导入的条件。
每个条件仍须通过正常校验。`RequestAccessUrlAllowList` 数据限制与引用侧的
`maxIpRules` 校验仍是独立约束。

一个集合必须且只能包含一个顶层运算符：

- `allOf` 只能直接包含 Condition，全部匹配时集合才匹配。
- `anyOf` 可以包含直接 Condition，或一层 `{ name?, allOf: [...] }` 分组；任一候选匹配时集合即匹配。

这套有界 DNF 语法支持扁平 AND、扁平 OR，以及 `(A AND B) OR C`：

```yaml
conditions:
  run:
    anyOf:
      - type: directPeerIpMatch
        ipRefs: [{ name: private-networks }]
      - name: authenticated-production
        allOf:
          - type: keyMatch
            key: { type: header, name: X-Env }
            values: [production]
          - type: keyExist
            key: { type: header, name: X-Authenticated-User }
```

该语法刻意不递归。所有数组必须非空，同一集合不能同时配置两个运算符；
`allOf -> anyOf`、`anyOf -> anyOf`、嵌套 `allOf -> allOf`、更深层嵌套、
未知容器字段，以及同一候选同时包含 `type` 与 `allOf` 都会被拒绝。
可选分组 `name` 不能为空白，且只影响归因，不改变布尔语义。

### Body 门闩与染色规则

条目级 `conditions`、支持该能力的 `body.conditions` 和染色规则的 `conditions`
都使用同一套 `PluginConditions` 动作容器。例如，下面的 body 门闩只对可能携带请求体的方法生效：

```yaml
body:
  maxBodySize: 1MiB
  conditions:
    run:
      anyOf:
        - type: keyMatch
          key: { type: method }
          values: [POST, PUT, PATCH]
```

body 条件只决定通用 body 门闩是否应用，不会跳过内部插件。部分固有 fail-close
请求体插件会刻意拒绝 `body.conditions`，具体边界见对应插件页面。染色条件只决定
已经匹配的结果是否渲染请求头。WebhookKeyGet 使用普通条目级条件，但不会因此获得
body 门闩；它的 outbound webhook body 与被代理的客户端请求体无关。

## 变体

| `type` | 含义 |
|--------|------|
| `keyExist` | 键存在则为真。 |
| `keyMatch` | 键的值落在 `values` 中，**或**匹配 `regex`，则为真。`negate: true` 是 exclude 的写法。 |
| `timeRange` | 当前时间不早于 `after` 且严格早于 `before`（RFC3339）则为真。任一边界都可省略。 |
| `probability` | 以 `ratio`（0.0–1.0）的概率为真。可选 `key` 使抽样确定化。 |
| `directPeerIpMatch` | 直接 TCP 对端落在 `ipRefs` 引用的 IpList 中则为真。 |
| `remoteIpMatch` | RealIp 解析出的客户端落在 `ipRefs` 引用的 IpList 中则为真。 |
| `secretMatch` | 请求键的值等于指定的 Secret data 值则为真；`secretKey` 指定 data 项。 |
| `hmacAuth` | 请求通过仅校验请求头的 HMAC 验证则为真。HMAC 字段（`secretGroups`、`algorithms` 等）与 `type` 同级。 |
| `jwtClaimMatch` | 已存储的 JWT claim 命中 `values` **或** `regex` 则为真。`claim` 是点分嵌套路径。 |
| `mtlsMatch` | mTLS 客户端证书字段命中则为真。`field` 为 `subject`、`cn` 或 `san`。 |

没有独立的 `include` / `exclude` 类型。原先的 `include` 写成 `keyMatch`；原先的 `exclude` 写成带 `negate: true` 的 `keyMatch`。

`hmacAuth.validateRequestBody` 在条目级 `run` / `skip`、body 门闩和染色条件上会被拒绝。它在 RequestAccessPolicy 与 RequestRestriction 的 ConditionSet 中合法。普通的请求值缺失或 HMAC 校验失败以 false 为基值，然后再应用 `negate`。ConfigData 或 Secret 引用未解析是另一种状态：它始终不匹配，`negate` 也不会将其反转。

## ConfigData 与 Secret 引用

`directPeerIpMatch` / `remoteIpMatch` 只接受 `ipRefs`，指向 IpList 类型的 ConfigData。IP/CIDR 写在 IpList 的 `items[].cidrs`，业务自己的小列表也可用一个 IpGroup。Gateway 统一编译每份 IpList，在一次调度中借用同一快照的 matcher；仅更新 IpList 即可改变保留的插件行为，不把地址复制到插件。Controller 负责引用授权，并解析 `secretMatch` 的 `secretRef` / `secretKey` 及 HMAC 的 `secretGroups[].secretRefs`；只有凭据仍写入 Condition 的内部解析字段。

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
- `field: san` — 任一 SAN 条目命中即满足。可选 `sanType`（`DNS`、`URI`、`Email`）按类型过滤并去掉存储前缀（`uri:` / `email:`）。`sanType: DNS` 只匹配无前缀条目，因此排除 `IP:`、`uri:`、`email:` 和 `otherName:`。未指定类型时匹配原始存储字符串。

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
RequestRestriction 集合还是 RequestAccessPolicy 补充集合中，都会执行同一校验。
如果写了 `values`，即使同时提供了非空 `regex`，`values` 也不能是空列表。

## `caseSensitive`

布尔，默认 **true**。只作用于 **`values`**。正则的大小写折叠写在模式里（`(?i)`）。

当 `caseSensitive: false` 时，精确匹配使用仅 ASCII 的折叠（`eq_ignore_ascii_case`）。超过 16 个值的列表会编译进 HashSet，折叠规则相同。

## 路径规范化

`keyMatch` 在 `key.type: path` 时，会在**加载时规范化精确 `values`**，使其与请求路径处于同一空间。这对 conditions 是新行为；`RequestRestriction` 对 `Path` 源已经这样做。

`key.type: requestPath` 是入口时的**原始**路径，**从不**规范化。用它匹配客户端实际发送的形态（`/%61dmin` 保持 `/%61dmin`；`path` 看到的是 `/admin`）。

正则**从不**规范化。请按实际观察到的路径形态编写正则。

## 编译后的描述符（访问日志）

每个编译后的条件都有仅运行时存在的描述符，包含它在集合中的稳定位置、
Condition 类型，以及适用时的访问器。位置能够区分直接叶子与嵌套分组，例如
`run.allOf[1]:keyMatch:hdr:X-Env` 或等价的紧凑线格式。描述符从不包含匹配值、
正则、CIDR、claim 或密钥。

只有当一个逻辑决策范围包含超过一个原子 Condition 时，才输出仅表示匹配成功的
原子描述符。插件条件会合并计算 `skip` 与 `run` 的叶子数；RequestRestriction
按完整集合计算；RequestAccessPolicy 则按每个候选的补充集合分别计算。
零个或一个叶子的结果已能从插件或策略最终动作中看出，重复记录只会增加噪声；
复合范围仍按配置求值顺序保留命中描述符，以便还原成功分支或前缀。

该阈值不会抑制 `cond_skip`、最终 allow/deny/reject、命名分组归因、
未解析引用或配置错误，以及 body 读取错误。body 阶段和染色求值无论叶子数多少都保持静默。

## 示例

```yaml
conditions:
  run:
    allOf:
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

- [请求限制](./request-restriction.md) — 使用相同 ConditionSet 的独立 allow/deny 插件
- [请求访问策略](./request-access-policy.md) — 带补充 ConditionSet 的七层策略
- [染色](./dye.md) — 条目级染色规则复用此 `conditions` 块
## `rateLimitExceeded`

在请求阶段 `RateLimitRedis` 条目的 `conditions.run` 中使用
`{"type":"rateLimitExceeded","pluginAlias":"local-budget"}`。alias 必须指向同一
资源 `requestPlugins` 数组内、顺序在前、处于启用状态且配置了
`onLimitExceeded: Continue` 的 `RateLimitLocal`。不能引用其他资源或阶段。
`skip`、body gate、dye 及其他消费者不支持此谓词。无效的活动条件会导致配置校验
失败，资源在运行时拒绝请求；禁用条目继续保持不执行、不校验其内部条件的语义。

本地超额时条件为真，本地通过时为假。如果本地条目被自己的条件跳过，或者缺 key
且 `onKeyMiss: Allow` 没有使用默认 key，则没有额度判定结果；活动的依赖消费者
返回通用 500，不把缺失状态当作假。此检查先于 skip/allOf/anyOf 短路求值。
除此之外维持原有布尔语义，其他有效条件仍可主动跳过 Redis。结果按资源和本次
插件链执行隔离，热更新不会混用结果。参见[完整补充限流示例](./rate-limit-redis.md#本地额度与-redis-补充准入)。

## 引用 IP 数据

IP/CIDR 集合只写在 IpList 资源中。业务自己的小列表可使用一个 IpGroup，多组仍属于同一份引用 matcher。没有聚合阈值，插件只携带引用。已删除的内联字段即使同时有有效引用也会判为无效。

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: private-networks
spec:
  data:
    type: IpList
    config:
      items:
        - name: private-clients
          cidrs: [10.0.0.0/8]
```
