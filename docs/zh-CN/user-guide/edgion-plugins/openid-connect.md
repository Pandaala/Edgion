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
| `requiredScopes` | Array\<String\> | 无 | 必需的 scope 列表，token 中缺少则拒绝 |
| `realm` | String | `"edgion"` | `WWW-Authenticate` 响应头中的 realm 值 |

### 认证流程参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `usePkce` | Boolean | `false` | 是否启用 PKCE（Proof Key for Code Exchange） |
| `useNonce` | Boolean | `false` | 是否在认证请求中包含 nonce |
| `redirectUri` | String | 无 | 自定义回调 URI，须为绝对路径或绝对 URL |
| `authorizationParams` | Object | 无 | 附加到授权请求的额外参数（不可包含保留键如 `response_type`、`client_id` 等） |
| `revokeTokensOnLogout` | Boolean | `false` | 登出时是否调用 Token Revocation 端点 |
| `logoutPath` | String | `"/logout"` | 触发登出的路径，须以 `/` 开头 |
| `postLogoutRedirectUri` | String | 无 | 登出后重定向 URI |

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
| `sslVerify` | Boolean | `true` | 是否验证 OIDC Provider 的 TLS 证书 |
| `issuers` | Array\<String\> | 无 | 允许的 issuer 列表 |
| `audiences` | Array\<String\> | 无 | 允许的 audience 列表 |
| `clockSkewSeconds` | Integer | `120` | 时钟偏差容差（秒） |
| `jwksCacheTtl` | Integer | `300` | JWKS 缓存 TTL（秒） |
| `jwksMinRefreshInterval` | Integer | `10` | JWKS 最小刷新间隔（秒） |
| `introspectionEndpoint` | String | 无 | 自定义 Introspection 端点 URL |
| `introspectionEndpointAuthMethod` | Enum | `client_secret_basic` | Introspection 端点认证方式。可选值：`client_secret_basic` | `client_secret_post` |
| `introspectionCacheTtl` | Integer | `0` | Introspection 结果缓存 TTL（秒），`0` 禁用 |
| `tokenEndpointAuthMethod` | Enum | `client_secret_basic` | Token 端点认证方式。可选值：`client_secret_basic` | `client_secret_post` |

### Header 转发参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `setAccessTokenHeader` | Boolean | `false` | 是否将 Access Token 传给上游 |
| `setIdTokenHeader` | Boolean | `false` | 是否将 ID Token 传给上游 |
| `setUserinfoHeader` | Boolean | `false` | 是否将 Userinfo 传给上游 |
| `accessTokenInAuthorizationHeader` | Boolean | `false` | 是否将 Access Token 写入上游的 `Authorization: Bearer` 头 |
| `claimsToHeaders` | Object | 无 | Claims 到 Header 的映射（支持 dot-notation，如 `user.name`） |
| `storeClaimsInCtx` | Boolean | `false` | 是否将 Claims 存入上下文变量 |
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
    - plugin:
        type: OpenidConnect
        config:
          discovery: "https://idp.example.com/.well-known/openid-configuration"
          clientId: "my-api"
          bearerOnly: true
          verificationMode: JwksOnly
          unauthAction: Deny
      enabled: true
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
    - plugin:
        type: OpenidConnect
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
      enabled: true
```

## Secret 约束

- `clientSecretRef`：读取 `clientSecret` / `client_secret` / `secret`
- `sessionSecretRef`：读取 `sessionSecret` / `session_secret` / `secret`
- `sessionSecret` 建议至少 32 字节（用于 AES-256-GCM）

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
- `502 Failed to fetch OIDC discovery document`
  - `discovery` 地址不可达或 TLS 验证失败
- `Session cookie exceeds configured size limit`
  - 减少 claims/header 透传，或调低 session 内数据量

## 相关文档

- [JwtAuth 插件](../http-route/filters/edgion-plugins/jwt-auth.md) - JWT 令牌认证
- [ForwardAuth 插件](forward-auth.md) - 外部鉴权插件
- [JWE Decrypt 插件](jwe-decrypt.md) - JWE 解密认证
