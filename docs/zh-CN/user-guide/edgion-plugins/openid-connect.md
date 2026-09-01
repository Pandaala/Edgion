# OpenID Connect 插件

> **🔌 Edgion 扩展**
> 
> OpenidConnect 是 `EdgionPlugins` CRD 提供的 OIDC/OAuth2 认证插件，不属于标准 Gateway API。

`OpenidConnect` 是请求阶段认证插件，用于在请求进入 upstream 前完成 OIDC/OAuth2 认证。

## 已实现能力

- Bearer Token 验证（JWKS、本地 JWT 验签、Introspection）
- Authorization Code Flow（含可选 PKCE）
- State/Nonce 校验
- Session Cookie 管理（AES-256-GCM 加密）
- Session 内 access token 内存缓存（cookie 不持久化 access token）
- Token 刷新与 singleflight 并发控制
- 登出（本地清理、可选调用 revoke、可选 end_session 重定向）
- Claims 映射到 Header（dot-notation、注入防护、大小限制）

## 配置参数

### 基础参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `discovery` | String | —（必填） | OIDC Discovery 端点 URL，必须以 `http://` 或 `https://` 开头 |
| `clientId` | String | —（必填） | OAuth2 Client ID |
| `clientSecretRef` | Object | 无 | 引用包含 client secret 的 K8s Secret |
| `bearerOnly` | Boolean | `false` | 仅验证 Bearer Token，不启用浏览器登录流程 |
| `unauthAction` | Enum | `Auth` | 未认证时行为。可选值：`Auth`（触发登录流） | `Deny`（返回 401） | `Pass`（放行） |
| `scope` | String | `"openid"` | OIDC 请求的 scope |
| `requiredScopes` | Array\<String\> | 无 | 必需的 scope 列表，token 中缺少则拒绝。**仅在 `bearerOnly: true` 时强制校验**（见下方说明） |
| `realm` | String | `"edgion"` | `WWW-Authenticate` 响应头中的 realm 值 |

> **关于 `requiredScopes` 的说明：** scope 强制校验仅在 bearer-only 模式
> （`bearerOnly: true`）下生效。在默认的 session/登录模式（`bearerOnly: false`，包括
> `unauthAction: Auth`）下，`requiredScopes` 在配置时会被接受，但运行时**不会**强制校验——
> 任何已认证用户都会通过，与 token 携带的 scope 无关。如需按 scope 限制访问，请将
> `bearerOnly` 设为 `true`。

### 认证流程参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `usePkce` | Boolean | `false` | 是否启用 PKCE（Proof Key for Code Exchange）。**生产部署强烈推荐 `true`**，详见[安全开关推荐配置](#安全开关推荐配置) |
| `useNonce` | Boolean | `false` | 是否在认证请求中包含 nonce。**生产部署强烈推荐 `true`**，详见[安全开关推荐配置](#安全开关推荐配置) |
| `redirectUri` | String | 无 | 自定义回调 URI，须为绝对路径或绝对 URL。路径形式使用请求的 `Host` 与监听器真实 TLS 状态，`X-Forwarded-Host` 与 `X-Forwarded-Proto` 均被忽略；当公开 origin 不同（含 TLS 在网关之前终止）时请使用绝对 URL |
| `authorizationParams` | Object | 无 | 附加到授权请求的额外参数（不可包含保留键如 `response_type`、`client_id` 等） |
| `revokeTokensOnLogout` | Boolean | `false` | 登出时是否调用 Token Revocation 端点 |
| `logoutPath` | String | `"/logout"` | 触发登出的路径，须以 `/` 开头 |
| `postLogoutRedirectUri` | String | 无 | 登出后重定向 URI。路径形式使用请求的 `Host` 与监听器真实 TLS 状态，`X-Forwarded-Host` 与 `X-Forwarded-Proto` 均被忽略；当公开 origin 不同（含 TLS 在网关之前终止）时请使用绝对 URL |

### Session 参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `sessionSecretRef` | Object | 无 | 引用包含 session 加密密钥的 K8s Secret（`unauthAction=Auth` 时必填） |
| `sessionCookieName` | String | `"edgion_oidc_session"` | Session Cookie 名称 |
| `sessionLifetime` | Integer | `3600` | Session 有效期（秒） |
| `sessionCookieSameSite` | String | `"Lax"` | Cookie SameSite 属性。可选值：`Strict` | `Lax` | `None` |
| `sessionCookieHttpOnly` | Boolean | `true` | Cookie HttpOnly 属性 |
| `sessionCookieSecure` | Boolean | `true` | Cookie Secure 属性（`SameSite=None` 时必须为 `true`） |
| `maxSessionCookieBytes` | Integer | `3800` | Session Cookie 最大字节数 |
| `renewAccessTokenOnExpiry` | Boolean | `true` | Access Token 过期时是否自动刷新 |
| `accessTokenExpiresLeeway` | Integer | `0` | Access Token 过期容差（秒） |

### Token 验证参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `verificationMode` | Enum | `Auto` | 验证模式。可选值：`Auto` | `JwksOnly` | `IntrospectionOnly` |
| `tokenSigningAlg` | String | 无 | 指定 Token 签名算法（不可为 `none`） |
| `allowedSigningAlgs` | Array\<String\> | 无 | 允许的签名算法列表（不可包含 `none`） |
| `useJwks` | Boolean | `true` | 是否使用 JWKS 进行本地验证 |
| `verify` | Boolean | `true` | 是否验证 OIDC Provider 的 TLS 证书。`false` 时跳过对端证书验证（由运维人员负责，启动时记录 WARN）。对于使用私有 CA 的 IdP，请配合 `validation.caCertificateRefs`（Secret/ConfigMap 引用列表，含 `ca.crt` 键）或 `validation.wellKnownCACertificates: System` 使用；`validation.subjectAltNames` 会增加 SAN 固定并强制进行完整证书验证。如需对 IdP 进行 mTLS 连接，请配合 `clientCertificateRef`（含 `tls.crt`/`tls.key` 的 Secret）。已声明的 TLS 材料如果未解析、只解析了一部分或格式错误，OIDC 插件构建会失败，受影响路由将 fail-close；系统绝不会回退到忽略声明策略的客户端。内联 CA（`caCert`）已不再支持。注意：上表「基础参数」中的 `clientSecretRef` 是 OAuth2 客户端密钥，与 TLS 配置无关。 |
| `issuers` | Array\<String\> | 无 | 允许的 issuer 列表 |
| `audiences` | Array\<String\> | 无 | 允许的 audience 列表 |
| `clockSkewSeconds` | Integer | `120` | 时钟偏差容差（秒） |
| `jwksCacheTtl` | Integer | `300` | JWKS 缓存 TTL（秒） |
| `jwksMinRefreshInterval` | Integer | `10` | JWKS 最小刷新间隔（秒） |
| `introspectionEndpoint` | String | 无 | 自定义 Introspection 端点 URL |
| `introspectionEndpointAuthMethod` | Enum | `client_secret_basic` | Introspection 端点认证方式。可选值：`client_secret_basic` | `client_secret_post` |
| `introspectionCacheTtl` | Integer | `0` | Introspection 结果缓存 TTL（秒），`0` 禁用 |
| `tokenEndpointAuthMethod` | Enum | `client_secret_basic` | Token 端点认证方式。可选值：`client_secret_basic` | `client_secret_post` |

### 内存缓存限制

只有 positive introspection 结果使用 TinyUFO/S3-FIFO。Access Token 间接存储
保留 LRU，因为 logout 和 refresh 需要显式失效：

```yaml
introspection:
  tinyUfoTtlCache:
    maxEntries: 2048
    maxLogicalBytes: 8MiB
accessToken:
  lruCache:
    capacity: 2048
    shards: 8
```

两类缓存分别继承 Gateway 的 `tiny_ufo_ttl_cache` 和 `lru_cache` 默认值。
覆盖 TinyUFO 配置时必须同时设置两个字段；省略两个字段则继承进程默认值。显式设置
LRU capacity 时，`shards: 0` 表示单分片；仅覆盖 shards 是无效配置。TinyUFO 字节
字段表示单个缓存实例的总逻辑记录权重预算，并非精确 RSS 上限。Discovery/JWKS 继续
使用专用 snapshot provider；refresh singleflight registry 也独立于两类缓存。

### Header 转发参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `setAccessTokenHeader` | Boolean | `false` | 是否将 Access Token 传给上游 |
| `setIdTokenHeader` | Boolean | `false` | 是否将 ID Token 传给上游 |
| `setUserinfoHeader` | Boolean | `false` | 是否将 Userinfo 传给上游 |
| `accessTokenInAuthorizationHeader` | Boolean | `false` | 是否将 Access Token 写入上游的 `Authorization: Bearer` 头 |
| `claimsToHeaders` | Object | 无 | Claims 到 Header 的映射（支持 dot-notation，如 `user.name`） |
| `storeClaimsInCtx` | Boolean | `false` | 是否将 Claims 存入上下文变量 `oidc_claims`（供下游插件读取；已从访问日志中排除，claims 不会被持久化） |
| `maxHeaderValueBytes` | Integer | `4096` | 单个映射 Header 值的最大字节数 |
| `maxTotalHeaderBytes` | Integer | `16384` | 所有映射 Header 的总最大字节数 |

### 安全参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `hideCredentials` | Boolean | `false` | 认证成功后是否移除原始 Authorization 头 |
| `authFailureDelayMs` | Integer | `0` | 认证失败延迟（毫秒），防止暴力破解 |

### 网络参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `timeout` | Integer | `3` | OIDC Provider 请求超时（秒） |

---

## 安全开关推荐配置

Edgion 的 `usePkce` / `useNonce` 两个开关默认为 `false`，与 Kong、APISIX、Envoy 等同类网关的默认行为一致（OIDC 规范对授权码流将二者定义为 OPTIONAL）。**但在生产部署中，强烈建议两者都设为 `true`**——它们提供的是 OIDC 授权码流的纵深防御，开销可忽略，主流 IdP（Keycloak、Auth0、Okta、Google、Azure AD 等）均原生支持。

### `useNonce: true`（防 ID Token 重放与跨会话注入）

- **缓解威胁**：攻击者从自己合法 OIDC 会话获取 ID Token，设法注入到受害者的 callback 流中绑定身份；或同一 Token 被多次回放
- **工作原理**：Edgion 在 `/authorize` 请求中带一个随机 `nonce`，存入签名的 state cookie；callback 拿到 ID Token 后校验 `nonce` claim 必须匹配
- **为何 Edgion 代码路径下威胁已被部分缓解**：授权码流 + 服务器侧 token exchange 让 ID Token 不经过浏览器，`state` 校验挡住了经典 CSRF；nonce 作为防御深化仍然有价值（防 IdP 侧重放、多客户端 token 混用等边缘场景）
- **启用代价**：IdP 必须把 nonce 回写到 ID Token 的 `nonce` claim。主流 IdP 自动支持；非标准 / 老旧 IdP 可能不支持——需先在测试环境验证

### `usePkce: true`（防授权码拦截）

- **缓解威胁**：公共客户端场景（SPA / 原生 App），授权码在 redirect_uri 上途中被第三方应用拦截并用于换取 Token
- **工作原理**：Edgion 在 `/authorize` 请求带 `code_challenge`（SHA-256(code_verifier)），callback 换 token 时带 `code_verifier`；IdP 验证二者绑定
- **适用范围**：即便 Edgion 是"机密客户端"（有 client_secret），启用 PKCE 仍是 OAuth 2.1 最佳实践，且对 IdP 是无感的额外绑定
- **启用代价**：IdP 必须支持 PKCE（RFC 7636）。主流 IdP 默认支持

### 推荐的最小生产配置

```yaml
config:
  usePkce: true      # 必开
  useNonce: true     # 必开
  sessionCookieSecure: true      # 默认已 true，保持
  sessionCookieSameSite: "Lax"   # 默认已 Lax，根据跨站需求可调
  verify: true                   # 默认已 true，切勿关闭
  verificationMode: Auto
```

### 何时可以不启用？

- **封闭内部环境 + bearer only 模式**（无 browser 交互、无 code flow）：`useNonce` / `usePkce` 不起作用，可保持 `false`
- **IdP 不支持 nonce / PKCE 且无法替换**（罕见）：保留 `false`，但必须通过其他方式弥补（如严格的 `sessionCookieSameSite=Strict` + IdP 侧的 referrer 校验）

---

## 快速开始

### 最小配置示例（Bearer Only）

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: oidc-api
  namespace: production
spec:
  requestPlugins:
    - type: OpenidConnect
      config:
        discovery: "https://idp.example.com/.well-known/openid-configuration"
        clientId: "my-api"
        bearerOnly: true
        verificationMode: JwksOnly
        unauthAction: Deny
```

## Web 登录示例（Code Flow + PKCE）

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: oidc-web
  namespace: production
spec:
  requestPlugins:
    - type: OpenidConnect
      config:
        discovery: "https://idp.example.com/.well-known/openid-configuration"
        clientId: "web-app"
        clientSecretRef:
          name: oidc-client-secret
        bearerOnly: false
        unauthAction: Auth
        usePkce: true
        useNonce: true
        sessionSecretRef:
          name: oidc-session-secret
```

## Secret 约束

- `clientSecretRef`：读取 `clientSecret` / `client_secret` / `secret`
- `sessionSecretRef`：读取 `sessionSecret` / `session_secret` / `secret`
- `sessionSecret` 必须至少 32 字节（用于 AES-256-GCM）

Controller 通常会解析这两个引用，并将值随插件 generation 下发。Gateway 在构造该
generation 时消费解析后的值；如果解析值缺失或为空，Gateway 仅在构造期执行一次旧式
Secret store 回退，请求路径不会重新读取 Secret。Session secret 会立即派生为该
generation 的 AES-256-GCM cipher，运行时插件配置不会保留原始 session-secret 字符串。
OAuth client secret 则作为一个不透明的 generation 值保留，仅用于向 IdP 进行客户端认证。

Secret 更新会在替换后的插件 generation 发布时生效；已开始的请求继续固定使用原有
generation。轮换 `sessionSecretRef` 会使上一 generation 加密的 cookie 失效，因此应预期
用户在 session secret 轮换后需要重新认证。

仅当当前模式和请求路径确实需要某个 Secret 时，Secret 缺失、为空、不是有效 UTF-8，或
session secret 少于 32 字节，才会在 `authFailureDelayMs` 延迟后返回相同的无细节
`401 Unauthorized` 响应和已配置的 Bearer challenge。基础的 `bearerOnly: true` +
`verificationMode: JwksOnly` 路径不需要 OAuth client secret；调用需要客户端认证的 IdP
端点时才需要。访问日志只记录固定失败分类，不包含 Secret 身份或内容。

IdP 传输、响应解码和请求期间的 TLS 失败属于独立的依赖故障，不受上述凭据 401 契约约束。
其 wire 响应遵循各端点自己的错误映射；应检查访问日志中的失败分类，而不是依赖统一的状态码
或响应体。已声明的 TLS CA 和 mTLS 材料由该 generation 的规范 outbound client 消费；
未解析或格式错误的声明材料遵循前述构造期 fail-close 行为，不会走上述认证凭据回退逻辑。

## 安全默认值

- 默认不透传 token 到 upstream
- Header 注入防护：拒绝 `\r`、`\n`、`\0`
- Header 大小限制：
  - `maxHeaderValueBytes`（默认 `4096`）
  - `maxTotalHeaderBytes`（默认 `16384`）
- Session cookie 大小限制：`maxSessionCookieBytes`（默认 `3800`）

## 常见排查

- `401 Unauthorized - Missing bearer token`
  - `bearerOnly=true` 且请求未带 `Authorization: Bearer ...`
- IdP 依赖故障
  - 传输、响应解码和 outbound TLS 失败使用各端点自己的状态码及响应体映射；请检查访问日志中的失败分类
- `Session cookie exceeds configured size limit`
  - 减少 claims/header 透传，或调低 session 内数据量

## 相关文档

- [JwtAuth 插件](../http-route/filters/edgion-plugins/jwt-auth.md) - JWT 令牌认证
- [ForwardAuth 插件](forward-auth.md) - 外部鉴权插件
- [JWE Decrypt 插件](jwe-decrypt.md) - JWE 解密认证
