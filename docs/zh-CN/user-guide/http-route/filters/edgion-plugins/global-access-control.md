# Global Access Control 插件

> **Edgion 扩展**
>
> GlobalAccessControl 是 `EdgionPlugins` CRD 提供的请求阶段放行规则插件，不属于标准 Gateway API。

## 概述

`GlobalAccessControl` 插件在 HTTP 请求阶段执行复合放行规则。请求只有满足**至少一个**配置的规则组才会被放行；每个规则组要求其所有条件（IP CIDR AND 请求属性规则）同时成立。

典型场景：**CDN 回源锁定（CDN/origin lockdown）**——源站只接受确实经过已知 CDN 转发的流量，判据是「来源 IP 落在 CDN 公布的 CIDR 段 **且** 带有 CDN 注入的密钥 header/cookie」。多个 CDN 可配置为独立规则组（组间 OR）；不匹配任何组的请求将被拒绝。

与 `IpRestriction`（仅 IP，无 profiles）或 `RequestRestriction`（仅请求属性，无 CIDR，无 profiles）不同，本插件集成了：
- IP/CIDR 匹配（基数树，O(log n)）
- 请求属性匹配（header、cookie、query、path、method），包含通过 K8s Secret 引用密钥的 **SecretRef**
- **身份匹配**——mTLS 客户端证书 subject / SAN / CN（`MtlsSubject` / `MtlsSan` / `MtlsCn`）以及已验证的 JWT claim（`JwtClaim`），使一个规则组可同时要求 IP **且** 经过证明的调用方身份
- 通过 `Selector` 类型 `EdgionConfigData` 实现的 profile 热切换
- 可配置的 fail-safe 默认行为

## 决策模型

```
遍历每个规则组（组间 OR）：
    组命中 ⟺ (未配置 ip 条件 OR remote_ip ∈ CIDR)
             AND 所有 rules 求值为 true

任一组命中 → 放行（GoodNext）
所有组不命中 → 执行 profile 的 defaultAction：
    Allow → 放行
    Deny  → 以配置的 status/message 拒绝
```

组内规则为 AND，组间为 OR（析取范式，DNF）。

## 三种挂载方式

GlobalAccessControl（以及所有 `EdgionPlugins`）可在三个粒度上挂载，按从宽到窄的顺序依次执行：

| 粒度 | 挂载方式 | 作用范围 |
|------|----------|----------|
| **GatewayClass 级** | `EdgionGatewayConfig.spec.globalPluginsRef` | 该 GatewayClass 下所有 HTTP 路由 |
| **Gateway 级** | Gateway 注解 `edgion.io/edgion-plugins: "ns/name, ..."` | 该 Gateway 下所有 HTTP 路由 |
| **HTTPRoute 级** | `HTTPRoute` filter `ExtensionRef` | 仅匹配的路由 |

三层均会依次执行，任意一层均可终止请求。

## 配置参数

### GlobalAccessControlConfig（顶层）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enable` | Boolean | `true` | 紧急 kill-switch。`false` 时整体禁用插件（pass-through），access-log 记录 `disabled:pass`。 |
| `activeProfile` | String | **必填** | 基线/默认 profile 名，必须存在于 `profiles` 中。 |
| `profiles` | Map\<String, AllowProfile\> | **必填** | 命名 profile 集合，支持通过 `activeProfileRef` 热切换。 |
| `activeProfileRef` | EdgionConfigDataRef | 无 | 指向 `Selector` 类型 `EdgionConfigData` 的引用，在运行时覆盖当前生效的 profile。解析失败时回落到 `activeProfile` 并记录 `selector miss: base`。 |
| `status` | Integer | `403` | 拒绝时返回的 HTTP 状态码。 |
| `message` | String | 无 | 拒绝时返回的可选响应体。 |
| `ipSource` | Enum | `DirectPeerIp` | CIDR 匹配时使用的 IP 来源。`DirectPeerIp` = 物理传输对端；`RemoteIp` = PROXY/RealIp/XFF 解析后的客户端（见安全说明）。 |

### AllowProfile

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `defaultAction` | Enum | `Deny` | 没有任何规则组命中时的动作。`Allow` = 放行（开放基线）；`Deny` = 以 `status`/`message` 拒绝（锁定）。 |
| `ruleGroups` | Array\<AllowRuleGroup\> | `[]` | 规则组列表，组间 OR。列表为空时没有任何组能命中，结果始终由 `defaultAction` 决定。 |

### AllowRuleGroup

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | String | 无 | 规则组名称，命中时记录在 access-log 中（`allow:group=<name>`）。 |
| `ip` | Array\<IpGroup\> | 无 | 可选的 IP CIDR 前置条件。省略时，任何来源 IP 均满足此条件。一个组只有在客户端 IP 落入至少一个 IpGroup 的 CIDR 时才满足 IP 条件。 |
| `rules` | Array\<RestrictionRule\> | `[]` | 请求属性断言，断言间 AND。 |

### IpGroup

| 参数 | 类型 | 说明 |
|------|------|------|
| `name` | String | 组名（便于阅读）。 |
| `cidrs` | Array\<String\> | CIDR 块，标准表示法，例如 `"173.245.48.0/20"`。 |

### RestrictionRule（断言）

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `source` | Enum | **必填** | 值来源：`Header` / `Cookie` / `Query` / `Path` / `RequestPath` / `Method` / `Referer` / `MtlsSubject` / `MtlsCn` / `MtlsSan` / `JwtClaim` |
| `key` | String | 无 | 键名。`Header`、`Cookie`、`Query` 时必填；`JwtClaim` 为点分隔 claim 路径；其余来源忽略。 |
| `values` | Array\<String\> | 无 | 精确匹配值；取到的值在列表中时断言为真。 |
| `regex` | Array\<String\> | 无 | 正则模式；取到的值匹配时断言为真（多个模式用 `\|` 合并）。 |
| `sanType` | Enum | 无 | 仅对 `MtlsSan` 有效。将匹配限定到某一 SAN 类型：`DNS` / `URI` / `Email`。省略时对原始 SAN 条目做整串匹配。 |
| `secretRef` | SecretObjectReference | 无 | K8s Secret 引用。Secret 中所有字符串值均成为可接受 token（并集），用于 CDN 密钥 header/cookie。 |
| `caseSensitive` | Boolean | `true` | `values` 精确匹配是否大小写敏感。 |
| `onMissing` | Enum | `NoMatch` | 值缺失时的断言结果：`Match` / `NoMatch` / `Skip`。**含 `secretRef` 的规则在校验时强制为 `NoMatch`**（见安全说明）。 |

> **值匹配并集**：`values`、`regex`、`secretRef` 解析出的值三者取并集求值——任一来源命中即该断言为真。

> **secretRef 注意**：所引用的 Secret 应专用于该凭据。Secret 中所有键的字符串值都会成为可接受 token（以支持密钥轮换）。密钥值**按原始字节精确、大小写敏感匹配，不做任何裁剪**——带换行符的值将永远无法匹配。

### 身份数据源

除请求属性与 `secretRef` 外，`rules` 断言还能匹配**认证后身份**——mTLS 客户端证书与已验证的 JWT claim。由于组内 rules 为 AND，可表达「IP 落在 CDN CIDR **且** SPIFFE URI SAN 等于 X」之类条件。身份值缺失时按 `onMissing`（默认 `NoMatch`）处理，因此证书/claim 缺失会使该断言为假，该组不命中。

#### mTLS 数据源（`MtlsSubject` / `MtlsCn` / `MtlsSan`）

- `MtlsSubject`——客户端证书完整的 subject **DN 字符串**。
- `MtlsCn`——仅 subject **CN**。
- `MtlsSan`——证书 **SAN** 列表，按「任一条目命中」求值。可选 `sanType`（`DNS` / `URI` / `Email`）限定匹配范围：
  - **不带类型**（省略 `sanType`）：对原始 SAN 条目（含类型前缀，如 `uri:spiffe://…`、`email:alice@example.com`；DNS 条目无前缀）做整串匹配。
  - **带类型**：仅考虑该类型的 SAN 条目，并对**去掉前缀后的值**匹配（如 `sanType: URI` 匹配 `spiffe://…`，而非 `uri:spiffe://…`）。

> **暴露开关（非对称）。** 客户端证书只有被暴露到插件层后，本插件才可见：
> - 在 **EdgionTls** 路径上，需要 `edgion.io/expose-client-cert` 注解才会暴露；否则 mTLS 数据源始终**缺失**（→ `onMissing`）。
> - 在 **Gateway `frontendValidation` mTLS** 路径上，证书**无需任何注解**即被暴露。
>
> 若证书未被暴露（或连接不是 mTLS），所有 mTLS 数据源都按缺失处理。

> **优先使用 `MtlsCn` / `MtlsSan`，而非 `MtlsSubject`。** `MtlsSubject` 匹配的是网关特定的 DN 序列化形式（如 `CN=foo, O=bar`——属性短名、`, ` 连接、按证书条目顺序）。它**对顺序和空格敏感**，细小的格式差异即会破坏精确匹配。`MtlsSubject` 建议仅用于 `regex` 匹配；精确身份校验请用 `MtlsCn` 或带类型的 `MtlsSan`。

#### JWT claim（`JwtClaim`）

- `key` 是**点分隔的 claim 路径**，如 `realm_access.roles` 或 `sub`。
- **列表**类 claim 按「任一元素命中」匹配；标量 string/number/bool claim 产生单个候选值。
- 取值来自上游认证插件写入请求上下文的**已验证** JWT claim：`jwt_auth` 写入 `jwt_claims`，`openid_connect` 写入 `oidc_claims`。查找优先级为先 `jwt_claims` 后 `oidc_claims`：存在哪一块就整块使用，不会对单个 claim 回退到另一块。
- **插件链顺序要求。** 认证插件（`jwt_auth` 或 `openid_connect`）必须在链中**更早执行**且设置 `store_claims_in_ctx: true`；否则上下文中没有 claim，`JwtClaim` 数据源始终**缺失**（→ `onMissing`）。

> **超出范围：`rules` 中的源 IP / CIDR 匹配。** IP/CIDR 匹配由规则组专用的 `ip` 字段处理，而非 `rules` 断言。出于同样原因，`sanType` 有意不提供 `IP`。

### EdgionConfigDataRef（activeProfileRef 使用）

| 参数 | 类型 | 说明 |
|------|------|------|
| `name` | String | `EdgionConfigData` 资源名（类型须为 `Selector`）。 |
| `namespace` | String | 命名空间，默认与 `EdgionPlugins` 相同。 |

被引用的 `EdgionConfigData` 必须是 `Selector` 类型，其 `config.active` 字段指定当前生效的 profile：

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: origin-lockdown-selector
  namespace: edgion-system
spec:
  data:
    type: Selector
    config:
      active: lockdown   # 改为 "normal" 即可一键解锁，无需重部署
```

## Access-log 决策记录

插件每次请求写入一条决策到 access-log 对应的插件槽（`stage_logs[].filters[].log[]`）：

| 决策字符串 | 含义 |
|-----------|------|
| `allow:group=<name>` | 请求命中了该规则组——放行。 |
| `allow:default` | 没有组命中，但 `defaultAction: allow`——放行。 |
| `deny:no-match` | 没有组命中，且 `defaultAction: deny`——拒绝。 |
| `disabled:pass` | 插件已禁用（`enable: false`）——pass-through。 |
| `selector miss: base` | `activeProfileRef` 解析失败，已回落到 `activeProfile`。 |
| `init fail: fail-open` | 插件初始化失败（gateway 启动时配置错误）——**所有流量放行**。见安全说明。 |

## CDN 回源锁定完整示例

场景：源站位于 Cloudflare 和 CloudFront 后面。只有来自已知 CDN CIDR 且携带 CDN 注入密钥 header 的请求才能到达后端，其余请求返回 403。

### Secret（每个 CDN 一个）

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: cf-origin-secret
  namespace: edgion-system
type: Opaque
stringData:
  # 无尾部换行符。此值必须与 X-CF-Origin-Secret 请求头完全一致。
  token: "my-cloudflare-secret-value"
---
apiVersion: v1
kind: Secret
metadata:
  name: cloudfront-origin-secret
  namespace: edgion-system
type: Opaque
stringData:
  token: "my-cloudfront-secret-value"
```

### Selector（用于热切换）

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: origin-lockdown-selector
  namespace: edgion-system
spec:
  data:
    type: Selector
    config:
      active: lockdown   # 改为 "normal" 即可禁用锁定，无需重新部署插件
```

### EdgionPlugins

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: origin-lockdown
  namespace: edgion-system
spec:
  requestPlugins:
    - type: GlobalAccessControl
      config:
        enable: true
        activeProfile: lockdown          # 基线；运行时由 activeProfileRef 覆盖
        status: 403
        message: "forbidden: origin accepts CDN traffic only"
        # ipSource 默认 DirectPeerIp（物理传输对端，不可伪造）——CDN 锁定必须保持默认值
        activeProfileRef:
          name: origin-lockdown-selector
          namespace: edgion-system
        profiles:
          # 开放基线——fail-to-base 回落于此，维护窗口时也可切换到此 profile
          normal:
            defaultAction: allow
            ruleGroups: []
          # 锁定——仅 Cloudflare 或 CloudFront + 各自的密钥
          lockdown:
            defaultAction: deny
            ruleGroups:
              - name: cloudflare
                ip:
                  - name: cf-v4
                    cidrs:
                      - "173.245.48.0/20"
                      - "103.21.244.0/22"
                      - "103.22.200.0/22"
                      - "103.31.4.0/22"
                      - "141.101.64.0/18"
                      - "108.162.192.0/18"
                      - "190.93.240.0/20"
                      - "188.114.96.0/20"
                      - "197.234.240.0/22"
                      - "198.41.128.0/17"
                      - "162.158.0.0/15"
                      - "104.16.0.0/13"
                      - "104.24.0.0/14"
                      - "172.64.0.0/13"
                      - "131.0.72.0/22"
                rules:
                  - source: Header
                    key: "X-CF-Origin-Secret"
                    secretRef:
                      name: cf-origin-secret
                      namespace: edgion-system
                    onMissing: NoMatch   # header 缺失 → 该规则为假 → 组不命中
              - name: cloudfront
                ip:
                  - name: cfront-v4
                    cidrs:
                      - "120.52.22.96/27"
                      - "205.251.249.0/24"
                      - "180.163.57.128/26"
                      - "204.246.168.0/22"
                rules:
                  - source: Header
                    key: "X-Origin-Verify"
                    secretRef:
                      name: cloudfront-origin-secret
                      namespace: edgion-system
                    onMissing: NoMatch
```

### 挂载到 Gateway（对该 Gateway 下所有 HTTP 路由生效）

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: public-gw
  namespace: edgion-system
  annotations:
    edgion.io/edgion-plugins: "edgion-system/origin-lockdown"
spec:
  gatewayClassName: edgion
  listeners:
    - name: http
      port: 80
      protocol: HTTP
```

如需仅对特定 HTTPRoute 生效，改用 `ExtensionRef`：

```yaml
# 在 HTTPRoute rule 的 filters 中：
filters:
  - type: ExtensionRef
    extensionRef:
      group: edgion.io
      kind: EdgionPlugins
      name: origin-lockdown
```

如需对整个 GatewayClass 生效（该 GatewayClass 下所有路由），在 `EdgionGatewayConfig` 中引用：

```yaml
# 在 EdgionGatewayConfig.spec 中：
globalPluginsRef:
  - name: origin-lockdown
    namespace: edgion-system
```

## 基于身份的放行示例（mTLS SAN 或 JWT claim）

当请求来自内网 CIDR **且**携带经证明的 SPIFFE URI SAN，**或**已验证 JWT 带有 `admin` 角色（无 IP 约束）时放行。两种情形是独立的规则组（组间 OR）；第一组内 `ip` 前置条件与 `MtlsSan` 断言为 AND。`JwtClaim` 分组生效的前提是认证插件（`jwt_auth` / `openid_connect`）在链中更早执行且设置 `store_claims_in_ctx: true`；`MtlsSan` 分组则需客户端证书已被暴露（见上文暴露开关）。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: identity-allow
  namespace: edgion-system
spec:
  requestPlugins:
    - type: GlobalAccessControl
      config:
        enable: true
        activeProfile: identity
        status: 403
        message: "forbidden: identity not allowed"
        profiles:
          identity:
            defaultAction: deny
            ruleGroups:
              - name: spiffe-checkout
                ip:
                  - name: internal-v4
                    cidrs:
                      - "10.0.0.0/8"
                rules:
                  - source: MtlsSan
                    sanType: URI
                    values: ["spiffe://edgion.io/ns/payments/sa/checkout"]
                    onMissing: NoMatch
              - name: jwt-admin
                rules:
                  - source: JwtClaim
                    key: "realm_access.roles"
                    values: ["admin"]
                    onMissing: NoMatch
```

## 执行流程

```mermaid
flowchart TD
    A["GlobalAccessControl 插件被调用"] --> B{"enable?"}
    B -->|false| Z1["log: disabled:pass → GoodNext"]
    B -->|true| C["通过 activeProfileRef 解析当前 profile\n（失败时回落到 activeProfile）"]
    C --> D["按 ipSource 获取客户端 IP\n（默认: DirectPeerIp = 物理传输对端）"]
    D --> E["遍历每个规则组（组间 OR）"]
    E --> F{"client IP ∈ group.ip CIDRs\n（或未配置 ip）？"}
    F -->|否| G["该组不命中 → 下一组"]
    G --> E
    F -->|是| H["逐条求值 rules（组内 AND）"]
    H --> I{"所有 rule 为真？"}
    I -->|否| G
    I -->|是| Z2["log: allow:group=<name> → GoodNext"]
    E -->|"所有组均不命中"| J{"defaultAction"}
    J -->|Allow| Z3["log: allow:default → GoodNext"]
    J -->|Deny| Z4["log: deny:no-match → 拒绝（status/message）"]
```

## 安全与运维说明

### 1. IP 来源：DirectPeerIp vs RemoteIp

`ipSource` 默认为 `DirectPeerIp`，读取 **物理传输对端地址**——即真实的网络层来源，客户端无法伪造。这是 CDN 锁定的正确选择，因为 CDN 始终从自己的 IP 段发起连接。

**仅在以下条件同时满足时才考虑 `ipSource: RemoteIp`**：
- Edgion Gateway 前置的是**可信的 L4 负载均衡器**（非 CDN 本身），且
- 同一路由上已正确配置 `RealIp` 插件，可从可信的 XFF 或 `X-Real-IP` 头提取真实 IP。

若在没有可信 L4 LB + 正确 RealIp 的情况下使用 `RemoteIp`，攻击者可以伪造 XFF 头，完全绕过 CIDR 检查。

### 2. 初始化失败时 fail-open（放行所有流量）

若插件在 gateway 启动时初始化失败（例如极端场景下某个格式错误的 CIDR 绕过了 controller 校验），插件**不会锁定流量**——而是**放行所有流量**并向 access-log 写入 `init fail: fail-open`。

这是有意为之的权衡：作为全局插件，初始化崩溃会中断该 Gateway/GatewayClass 下的所有流量。fail-open 行为保障了可用性，但也意味着错误配置会静默地禁用锁定。

**缓解措施**：对 access-log 中出现 `init fail: fail-open` 的条目设置告警。Controller 已在入站时拒绝格式错误的配置（`Accepted=False`），因此该路径仅在极端边缘场景（竞态、fed_sync 等）下才会触发。

### 3. Gateway 注解引用悬空（dangling ref）

若 Gateway 的 `edgion.io/edgion-plugins` 注解引用了一个不存在（或已删除）的 `EdgionPlugins` 资源，数据面将**静默跳过该插件**，流向该 Gateway 的流量**不受锁定保护**。

唯一的信号是 Gateway 资源的状态条件：
```
ResolvedRefs = False
reason: InvalidPluginsRef
```

**缓解措施**：对预期挂载了锁定插件的 Gateway 的 `ResolvedRefs=False` 设置告警。

### 4. SecretRef：专用 Secret，不含尾部空白

`secretRef` 引用的 K8s Secret 必须**专用于该凭据**。Secret 中所有键的字符串值（跨所有 key）都会成为可接受 token，这是有意设计——可通过先添加新值再删除旧值实现零停机密钥轮换。

注意事项：
- 不要与无关资源共享同一 Secret。
- **Secret 值不能含有尾部换行符或空白**。值按原始字节、精确、大小写敏感匹配。使用 `kubectl create secret --from-file` 会原样编码文件内容，若文件以换行符结尾，生成的 token 将永远无法匹配。建议使用 `--from-literal`，或手动校验 Secret 中的值。

### 5. 含 secretRef 的规则强制 onMissing: NoMatch

若规则包含 `secretRef`，controller 在校验时会强制将 `onMissing` 改为 `NoMatch`。若允许 `onMissing: Match` 或 `onMissing: Skip`，则在密钥 header 缺失时仅凭 IP 命中即可放行，完全破坏 CDN 锁定。该配置将被 admission 拒绝（`Accepted=False`）。

## Profile 热切换（Selector）

`activeProfileRef` 指向一个 `Selector` 类型的 `EdgionConfigData`，指定当前生效的 profile 名。更新 `EdgionConfigData` 会通过标准 conf_sync 路径立即触发热重载，无需重新部署或重启。

常见模式：
- **维护窗口**：将 selector 从 `lockdown` 切换到 `normal` → 所有流量放行 → 维护完成 → 切回。
- **渐进锁定**：先用 `defaultAction: allow` 的 profile 验证 CDN IP 段，确认无误后通过 selector 切换到 `lockdown`。
- **Fail-to-base**：Selector 解析失败（`EdgionConfigData` 缺失、类型错误、selector `enable=false`）时，插件回落到 `activeProfile` 并记录 `selector miss: base`。应将 `activeProfile` 设计为安全的默认值。

## 注意事项

1. **规则组求值顺序**：按配置顺序依次求值，第一个命中的组立即停止。
2. **空 ruleGroups**：`ruleGroups: []` 时没有任何组能命中，结果始终由 `defaultAction` 决定。`defaultAction: allow` + 空 `ruleGroups` 是常用的开放基线模式。
3. **省略 ip 字段**：组内无 `ip` 字段时，IP 条件对任何来源均满足——只需 `rules` 全部为真即可命中。
4. **多 IpGroup**：`ip` 数组内多个 `IpGroup` 之间为 OR——客户端 IP 落入任意一个 IpGroup 的 CIDR 即满足 IP 条件。
5. **values、regex、secretRef 取并集**：单条规则内三者取并集——任一来源命中即该断言为真。
6. **挂载执行顺序**：GatewayClass 插件最先执行，其次 Gateway 级，最后 route 级。任意一层均可终止请求。Gateway 级锁定插件早于 route 级插件执行。

## 常见问题

### Q1：如何临时放行所有流量而不移除插件？

将 `EdgionConfigData` selector 切换到 `defaultAction: allow` + `ruleGroups: []` 的 profile（例如示例中的 `normal`），数秒内通过热重载生效。

### Q2：如何同时支持多个 CDN？

为每个 CDN 添加一个规则组，各自配置对应的 `ip` CIDR 和 `rules`。组间 OR——任意一个 CDN 组命中即放行。

### Q3：可以不使用 SecretRef 仅做 IP 锁定吗？

可以，省略规则组的 `rules` 字段（或设为 `rules: []`），则仅 IP CIDR 条件生效。但纯 IP 锁定弱于 IP+密钥：若攻击者的源地址恰好在 CDN 的 CIDR 段内，将在无 SecretRef 的情况下绕过检查。

### Q4：activeProfileRef 引用的 EdgionConfigData 被删除后会怎样？

插件回落到 `activeProfile` 并记录 `selector miss: base`。Controller 同时在 `EdgionPlugins` 资源上设置 `ResolvedRefs=False`。

### Q5：如何确认插件正在运行？

检查 access-log 中的决策字符串（`allow:group=<name>`、`deny:no-match` 等）。正确配置锁定后，合法的 CDN 流量应记录 `allow:group=cloudflare`（或你配置的组名），直接访问应记录 `deny:no-match`。

---

## 获取帮助

如果遇到问题：

1. 检查上述 access-log 决策字符串。
2. 检查 Gateway 和 EdgionPlugins 的 status 条件（`ResolvedRefs`）。
3. 检查 gateway 日志中是否有关于 `init fail` 的 `warn` 条目。
4. 在 GitHub 上提交 Issue。

---

## 相关链接

- [IpRestriction](./ip-restriction.md) — 纯 IP 黑白名单（无 profiles，无密钥 header）
- [RequestRestriction](./request-restriction.md) — 请求属性过滤（无 CIDR，无 profiles）
- [RealIp](./real-ip.md) — 客户端身份解析（使用 `ipSource: RemoteIp` 时必须配合）
