# OpenID Connect 插件

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

## 最小配置示例（Bearer Only）

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
