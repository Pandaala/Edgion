# LDAP Auth 插件

> **🔌 Edgion 扩展**
> 
> LdapAuth 是 `EdgionPlugins` CRD 提供的 LDAP 目录认证插件，不属于标准 Gateway API。

## 概述

LDAP Auth 插件用于将网关认证委托给企业 LDAP/AD 目录服务。插件从请求头解析 `username:password`，再使用 LDAP Simple Bind 校验凭证。

适用场景：
- 统一账号体系（LDAP / Active Directory）
- 不希望在网关内维护本地用户密码
- 需要与企业现有账号生命周期管理一致

## 功能特性

- 支持 `Authorization` / `Proxy-Authorization` 认证头
- `Proxy-Authorization` 优先级高于 `Authorization`
- 支持自定义认证方案名（`headerType`，如 `ldap` / `basic`）
- 支持仅在缺少凭证时匿名放行（`anonymous`）
- 支持隐藏凭证头（`hideCredentials`）
- 支持 LDAPS / StartTLS（通过 `ldaps` / `startTls`）
- 搜索模式：由服务账号用 LDAP 过滤器查找用户条目，登录名无需出现在 DN 中
  （例如 Active Directory 的 `sAMAccountName`、分散在多个 OU 下的用户）
- 每次 LDAP 连接在拨号时应用网关出站流量策略

## 工作流程

1. 读取请求头：先读 `Proxy-Authorization`，再读 `Authorization`
2. 解析格式：`{headerType} base64(username:password)`
3. 构建 Bind DN：
   - 默认：`{attribute}={username},{baseDn}`
   - 模板：`bindDnTemplate` 替换 `{username}`
   - `{username}` 作为 DN 属性值，按 RFC 4514 转义（`, + " \ < > ; =` 等
     特殊字符、开头的 `#`、首尾空格都会被转成 `\HH` 十六进制形式）。因此该
     模板只能描述 DN，不能用作 search filter 模板。
4. LDAP Simple Bind 校验
5. 成功后注入上游头并继续转发

用户名中包含控制字符（包括 TAB 和 NUL）时，在发起任何 LDAP 请求之前直接返回
`401`。搜索模式下第 3、4 步由[搜索模式](#搜索模式)中的流程替代。

## 配置参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `ldapHost` | String | 无 | LDAP 主机，必填 |
| `ldapPort` | Integer | `389` | LDAP 端口 |
| `ldaps` | Boolean | `false` | 是否启用 LDAPS |
| `startTls` | Boolean | `false` | 是否启用 StartTLS（与 `ldaps` 互斥） |
| `tls.verify` | Boolean | 继承 | 是否验证 LDAP 服务器证书。未设置本地值时继承 `EdgionGatewayConfig.spec.outboundTls.verify`，最终默认值为 `true`。 |
| `tls.validation.caCertificateRefs` | Array | 继承 | CA 证书 Secret 引用。存在本地 `validation` 块时，会整体替换全局 validation 块。 |
| `tls.validation.subjectAltNames` | Array | 继承 | 附加 SAN 策略。非空策略会强制执行完整证书验证，即使请求了 `verify: false`。 |
| `tls.clientCertificateRef` | Object | 继承 | 客户端 mTLS 身份 Secret。本地客户端身份独立于 validation 块解析；未设置时继承全局身份。 |
| `baseDn` | String | 无 | 基础 DN，必填 |
| `attribute` | String | 无 | 用户属性，必填（如 `uid`/`cn`） |
| `bindDnTemplate` | String | 无 | 自定义 Bind DN 模板，必须包含 `{username}` |
| `bindDn` | String | 无 | 服务账号的**条目 DN**（不能是 UPN 或 `DOMAIN\user`），须为字符串形式：属性名用 `cn` 这类名字而非 OID，且不含 `#hex` 值。设置后进入搜索模式，与 `bindDnTemplate` 互斥 |
| `bindPasswordRef` | Object | 无 | 存放服务账号密码的 Secret，密码位于 key `password`；设置 `bindDn` 时必填 |
| `searchFilter` | String | `(<attribute>={username})` | 搜索模式：包含 `{username}` 的 RFC 4515 过滤器（最多 8 处 `{username}`，最长 4096 字节，嵌套深度最多 16）；扩展匹配规则必须写数字 OID（如 `1.2.840.113556.1.4.1941`），不能写名字 |
| `searchBase` | String | `baseDn` | 搜索模式：整棵子树搜索的起点 DN；它（未设置时为 `baseDn`）必须是合法 DN |
| `requiredGroupsAnyOf` | String[] | 无 | 搜索模式：组 DN 列表，用户至少属于其中一个（1 到 32 项，每项为完整 DN） |
| `requiredGroupsAllOf` | String[] | 无 | 搜索模式：组 DN 列表，用户必须属于全部（1 到 32 项）。与 `requiredGroupsAnyOf` 同时配置时两者都须满足 |
| `groupAttribute` | String | `memberOf` | 用户条目上列出组 DN 的属性；必须由目录维护，且用户不能修改 |
| `nestedGroups` | Boolean | `false` | 用 Active Directory 的链式规则 `1.2.840.113556.1.4.1941` 匹配嵌套组（OpenLDAP 不支持） |
| `headerType` | String | `ldap` | 认证头方案名 |
| `hideCredentials` | Boolean | `false` | 是否移除认证头再转发。若无法移除，请求在 `authFailureDelayMs` 之后以普通的 `401` 拒绝（访问日志记录 `credential removal failed`，诊断码 `AuthHeaderMutationRejected`），凭证不会被转发 |
| `authFailureDelayMs` | Integer | `0` | 返回凭证相关 401 响应前的延迟，单位为毫秒 |
| `anonymous` | String | 无 | 匿名用户名；仅当两个凭证头都缺失时生效 |
| `realm` | String | `API Gateway` | `WWW-Authenticate` realm; [字符规则](../http-route/filters/edgion-plugins/README.md#authentication-realm) |
| `timeout` | Integer | `10000` | LDAP 超时（毫秒），分别作用于建连阶段（DNS 解析、TCP、StartTLS、TLS 握手）和 bind 加 unbind 阶段（搜索模式下该阶段包含服务账号 bind、搜索、用户 bind，配置组要求时还包含服务账号重新 bind 和组搜索，以及 unbind）；一次 LDAP 交互最长为该值的两倍，返回 401 或 403 时另加 `authFailureDelayMs` |
| `keepalive` | Integer | `60000` | 保留字段 |
| `credentialIdentifierHeader` | String | `X-Credential-Identifier` | 认证用户名透传头（搜索模式下为条目中 `attribute` 的值） |
| `anonymousHeader` | String | `X-Anonymous-Consumer` | 匿名标记头 |

## 最小配置示例

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ldap-auth
  namespace: production
spec:
  requestPlugins:
    - enable: true
      type: LdapAuth
      config:
        ldapHost: ldap.example.com
        ldapPort: 389
        startTls: true
        baseDn: "dc=example,dc=com"
        attribute: "uid"
        timeout: 3000
```

LDAP 认证结果不缓存。每个带凭证的请求都会由权威 LDAP 服务校验。

## TLS、全局继承与警告

仅当 `ldaps: true` 或 `startTls: true` 时，Gateway 连接才使用 `tls`。对于启用
TLS 的 LDAP，未设置的本地 `tls.verify` 会继承全局
`EdgionGatewayConfig.spec.outboundTls.verify`。本地 `tls.validation` 块会整体
替换全局 validation 块，不会逐字段合并。客户端身份独立继承：本地
`tls.clientCertificateRef` 优先，否则使用已配置的全局身份。

```yaml
requestPlugins:
  - type: LdapAuth
    config:
      ldapHost: ldap.example.com
      ldapPort: 636
      ldaps: true
      baseDn: "dc=example,dc=com"
      attribute: uid
      tls:
        verify: true
        validation:
          caCertificateRefs:
            - kind: Secret
              name: ldap-ca
        clientCertificateRef:
          name: ldap-client-identity
```

TLS 警告分别表示以下含义：

- 实际使用跳过验证的最终配置时，共享出站 TLS 解析器会警告证书验证已禁用。
- 非空 SAN 策略会覆盖请求的 `verify: false`，记录覆盖警告，强制执行证书链和
  SAN 验证，并保留可用的 mTLS 身份。
- 在最终配置跳过验证的同时设置本地 `clientCertificateRef` 时，客户端身份会被
  有意省略，LdapAuth 会额外记录 M4 警告，说明客户端证书没有发送。

对于普通 LDAP（`ldaps: false` 且 `startTls: false`），Gateway 插件构造不会调用
出站 TLS 解析器、构建 TLS 客户端配置，也不会记录上述共享或 M4 警告。不过，
Controller 仍会处理所有已声明的 TLS Secret 引用、注册依赖、检查 Secret 和
ReferenceGrant，并可能更新状态、记录日志或重新入队插件。普通 LDAP 应省略未使用
的 TLS 配置；不要假设它在控制面完全无影响。

## 出站流量策略

每次 LDAP 连接都会在拨号时解析 `ldapHost`，并且只连接网关出站流量策略允许的地址。回环、链路本地和未指定地址始终被拒绝。RFC1918 私网地址遵循 `outbound_egress_block_private` / `EDGION_OUTBOUND_BLOCK_PRIVATE`（默认 `false`），因此除非打开该开关，典型的集群内 LDAP 仍然可达。被拒绝的目的地与目录不可达一样，以通用 503 失败关闭。LDAPS 和 StartTLS 仍校验原来的 `ldapHost` 主机名。详见[出站流量策略（SSRF 防护）](../../ops-guide/infrastructure/connectors.md#出站流量策略ssrf-防护)。

## 使用 `headerType: basic` 示例

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ldap-auth-basic-scheme
  namespace: production
spec:
  requestPlugins:
    - enable: true
      type: LdapAuth
      config:
        ldapHost: ldap.example.com
        baseDn: "dc=example,dc=com"
        attribute: "uid"
        headerType: "basic"
```

客户端可直接使用 Basic 认证：

```bash
curl -u alice:password123 https://api.example.com/protected
```

## 匿名降级示例

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ldap-auth-anon
  namespace: production
spec:
  requestPlugins:
    - enable: true
      type: LdapAuth
      config:
        ldapHost: ldap.example.com
        baseDn: "dc=example,dc=com"
        attribute: "uid"
        anonymous: "guest-user"
        hideCredentials: true
```

仅当两个凭证头都不存在时才会匿名放行。格式错误的认证头以及被 LDAP 拒绝的凭证
仍会返回 401。匿名放行时会注入：
- `X-Credential-Identifier: guest-user`
- `X-Anonymous-Consumer: true`

## 搜索模式

设置 `bindDn` 后，插件不再拼接 DN，而是查找用户。每个请求在同一条连接上依次：

1. 以服务账号 bind（`bindDn` 加 `bindPasswordRef` 中 key `password` 的值）；
2. 在 `searchBase` 下用 `searchFilter` 做整棵子树搜索（最多 2 条结果），只请求 `attribute`；
3. 要求恰好命中一个条目，再用客户端密码以该条目的 DN bind；
4. 把该条目 `attribute` 的唯一值（例如 `uid` 或 `sAMAccountName`）写入
   `credentialIdentifierHeader`，而不是客户端输入的字符串。

```yaml
config:
  ldapHost: "ad.corp.example.com"
  ldapPort: 636
  ldaps: true
  baseDn: "DC=corp,DC=example,DC=com"
  attribute: "sAMAccountName"
  bindDn: "CN=svc-gateway,OU=Service Accounts,DC=corp,DC=example,DC=com"
  bindPasswordRef:
    name: ldap-service-account     # Secret，key 为 "password"
  searchFilter: "(&(objectClass=user)(sAMAccountName={username}))"
  tls:
    validation:
      caCertificateRefs:
        - kind: Secret
          name: corp-ca
```

规则与保证：

- 搜索模式要求 `ldaps` 或 `startTls`，并且必须校验证书；本地设置 `tls.verify: false`
  且没有 SAN 策略时会被拒绝。继承自全局默认的 `verify=false` 只会在插件构建时打印告警。
- 用户名以原始字节写入已编译的过滤器，用户名中的 `*`、`(`、`)`、`\` 等字符无法改变
  过滤器结构（不存在 LDAP 注入）。
- 查不到条目、命中多个条目、条目没有恰好一个合法的 `attribute` 值（非空、首尾无空格、
  无控制字符）、条目 DN 无法解析、以及命中服务账号自身条目，都返回与密码错误相同的 `401`。`attribute` 必须是目录返回的名字（不解析
  `userid` 与 `uid` 之类的别名）；多值属性会拒绝该用户。
- 服务账号密码被拒绝或未解析时返回通用 `401`，并计入
  `edgion_ldap_auth_errors_total{kind="service_bind_fail"}`，请为它配置告警。
  `bindPasswordRef` 指向的 Secret 缺少可用的 `password` 值时，资源状态还会显示
  `ResolvedRefs=False`（`MissingKeys` 或 `DecodeError`）。
- 时序：不存在的用户会省去最后一次 bind，比密码错误快一次 LDAP 往返。
  `authFailureDelayMs` 能拖慢猜测，但无法抹平这一差异；请用 `RateLimitLocal` 限制探测。
- 服务账号需要具备在 `searchBase` 下搜索并读取 `attribute` 的权限。

### 组要求

配置 `requiredGroupsAnyOf` 和/或 `requiredGroupsAllOf` 后，密码正确的用户还必须属于指定的组：

```yaml
config:
  # ... 上面的搜索模式配置 ...
  requiredGroupsAnyOf:
    - "CN=API Users,OU=Groups,DC=corp,DC=example,DC=com"
    - "CN=API Admins,OU=Groups,DC=corp,DC=example,DC=com"
  requiredGroupsAllOf:
    - "CN=MFA Enrolled,OU=Groups,DC=corp,DC=example,DC=com"
  nestedGroups: true   # 仅限 Active Directory
```

- 只有用户 bind 成功之后，插件才会再次以服务账号 bind，并以用户自己的条目为起点做 base 范围
  搜索，过滤条件由 `(<groupAttribute>=<组 DN>)` 组成（`requiredGroupsAllOf` 要求全部满足，
  `requiredGroupsAnyOf` 要求至少一个）。密码错误时不会做组检查，照常返回 `401`。
- 组成员照常放行；非成员返回 `403 Forbidden`（同样先等待 `authFailureDelayMs`）。注意 401 与
  403 的区别会让客户端知道非成员的密码是否正确。如果介意，请把组条件写进 `searchFilter`，
  这样非成员会得到与密码错误相同的 `401`。
- 组用完整 DN 表示，由目录负责比较。已存储资源中的空列表（`[]`）、`null` 值或 `null` 列表项
  （例如模板渲染出的空值）会被拒绝，并在资源状态中报错。注意：用 `kubectl apply`、Helm 或 Argo CD
  更新已有资源时，清单里设为 `null` 的字段会作为 merge patch 删除操作发送，该键会被直接删掉，
  组要求随之失效且不会报错。请只在确实需要时移除组要求，并检查应用后的资源。拼错的键名（例如
  `requiredGroupAnyOf`）同样不会被插件拒绝：应用配置后请检查资源状态中的未知字段告警。
- **前提：`groupAttribute` 由目录维护，且用户无法修改。** Active Directory 的 `memberOf` 是
  系统维护的反向链接。OpenLDAP 需要 `memberof` overlay（配合 `refint`），并且它的 `memberOf`
  默认可被用户修改，必须用 ACL 保护——在任何 `by self write` 规则之前加入类似
  `access to attrs=memberOf by * read` 的规则。直接写入的 `memberOf` 值同样算作成员关系。
  不要把 `groupAttribute` 指向用户可编辑的属性（例如 `description`）。
- 服务账号需要能读取用户条目上的 `groupAttribute`，否则所有用户都会得到 `403`。
- Active Directory：主组（通常是 `Domain Users`）不会出现在 `memberOf` 中。`nestedGroups: true`
  使用链式规则，组关系很大时可能较慢；OpenLDAP 不实现该规则，成员也会得到 `403`。
- 代价：每次成功登录多一次 bind 和一次搜索。

## 错误语义

| 场景 | 状态码 | 说明 |
|------|--------|------|
| 缺少凭证（且未启用 `anonymous`） | `401` | 返回 `WWW-Authenticate` |
| 凭证格式非法 | `401` | 通用认证失败 |
| LDAP 凭证错误 | `401` | 通用认证失败 |
| LDAP 服务不可达/超时 | `503` | 服务不可用 |
| 服务器拒绝 StartTLS（结果码非 0） | `503` | 计入 `starttls_rejected`，与 `unreachable` 区分 |
| LDAP 目的地被出站流量策略拒绝 | `503` | 与不可达相同的通用响应体 |
| 用户名包含控制字符 | `401` | 在发起任何 LDAP 请求之前拒绝 |
| 搜索模式：用户名超过 1024 字节 | `401` | 在发起任何 LDAP 请求之前拒绝 |
| 组要求：密码正确但不是成员 | `403` | `403 Forbidden`；只在用户 bind 成功后检查 |
| 搜索模式：用户不存在、有歧义或条目不合规 | `401` | 与密码错误相同 |
| 搜索模式：服务账号密码被拒绝或未解析 | `401` | 计入 `service_bind_fail` |
| 搜索模式：搜索失败（referral、搜索起点不存在、协议错误、目录对该用户名报错；配置组要求时还包括组搜索失败，或服务账号重新 bind 因密码被拒以外的原因失败） | `503` | 计入 `search_fail` |

## 安全建议

- 生产环境优先使用 `ldaps: true` 或 `startTls: true`
- 保持最终生效的 `tls.verify: true`；`false` 由运维人员负责，仅应用于测试环境
- 开启 `hideCredentials: true`，避免凭证透传到上游
- 在 LDAP 服务器侧启用账户锁定与防爆破策略
- 叠加 `RateLimitLocal` 插件，并按受保护请求速率规划 LDAP 服务容量

> **可接受的权衡——每请求新建 LDAP 连接。** 每次认证都会新建一条 TCP（必要时附带 TLS）连接，执行 `simple_bind`（搜索模式下依次为服务账号 bind、搜索、用户 bind，配置组要求时再加服务账号重新 bind 和组搜索）后立即 `unbind`。当前实现不引入连接池，因为 LDAP `simple_bind` 会改变连接的认证身份；不安全的跨用户复用比连接开销更危险。

## 相关文档

- [BasicAuth 插件](../http-route/filters/edgion-plugins/basic-auth.md) - 基础用户名密码认证
- [ForwardAuth 插件](forward-auth.md) - 外部鉴权插件
- [JwtAuth 插件](../http-route/filters/edgion-plugins/jwt-auth.md) - JWT 令牌认证
