# JWT Auth 插件

> **🔌 Edgion 扩展**
> 
> JwtAuth 是 `EdgionPlugins` CRD 提供的认证插件，不属于标准 Gateway API。

## 什么是 JWT Auth？

JWT Auth（JSON Web Token 认证）是一种基于令牌的认证机制，客户端在请求中携带 JWT，由网关验证签名和声明后放行。

**工作原理**：
1. 客户端发送带有 JWT 的请求（通过 Header、Query 或 Cookie）
2. 插件解码 JWT 的 `iss`（颁发者）声明，路由到对应的 issuer 组
3. 验证 JWT 签名、过期时间等声明（使用该组的算法和密钥）
4. 验证成功：允许访问，并记录 access log `OK group=<name> iss=<iss> u=<sub>`；`sub` 会按照敏感数据日志策略进行掩码处理
5. 验证失败：返回 401 状态码

**与 BasicAuth 的区别**：
- BasicAuth：每次请求传输用户名和密码
- JwtAuth：只验证令牌，不传输密码；支持无状态认证；令牌有过期时间

## 快速开始

### 最简单的配置

```yaml
# EdgionPlugins 配置
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: jwt-auth-plugin
  namespace: default
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        algorithm: HS256
        issuerGroups:
          - name: default
            issuers:
              - "https://my-idp.example.com"
            secretRefs:
              - name: jwt-secret
```

### 创建 Kubernetes Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: jwt-secret
  namespace: default
type: Opaque
stringData:
  key: "my-key-id"
  secret: "my-jwt-secret-key-32-chars-long!!"
```

---

## 配置参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `issuerGroups` | Array | 无 | Issuer 分组列表（见下方子表）。未设置 `anonymous` 时必须配置 |
| `algorithm` | String | `HS256` | 插件级默认签名算法，可被每个 issuerGroup 覆盖 |
| `audiences` | Array | 无 | 插件级默认有效受众列表，可被每个 issuerGroup 覆盖 |
| `header` | String | `authorization` | 从哪个 Header 读取 Token（支持 `Bearer <token>` 或裸 token） |
| `query` | String | `jwt` | Query 参数名。仅当 `allowTokenInQuery` 为 `true` 时才会读取 |
| `allowTokenInQuery` | Boolean | `false` | 是否允许从 URL query 参数读取 token。默认禁用，因为 URL 中的 bearer token 会泄漏到 access log、浏览器历史和 `Referer` 头（RFC 6750 §2.3/§5.3） |
| `cookie` | String | `jwt` | Cookie 名称 |
| `hideCredentials` | Boolean | `false` | 是否隐藏 Token，不传递给上游服务 |
| `anonymous` | String | 无 | 匿名用户名。设置后，仅未携带 Token 的请求会被允许并写入匿名请求头；携带无效 Token 仍返回 401 |
| `keyClaimName` | String | `key` | JWT payload 中用于选择密钥的 claim 名称 |
| `lifetimeGracePeriod` | Integer | `0` | exp/nbf 时钟偏差容忍（秒） |
| `base64Secret` | Boolean | `false` | `false` 直接使用 Controller 提供的 UTF-8 `secret` 字节；`true` 在 Kubernetes 已解码 Secret `.data` 外层后，再把该字符串按 standard Base64 解码 |
| `maximumExpiration` | Integer | `0` | 最大允许的 exp 时长（秒，从当前时间起）。`0` 表示不限制 |
| `authFailureDelayMs` | Integer | `0` | 返回认证失败响应前的延迟（毫秒），同时作用于缺少 Token 和 Token 无效的情况 |
| `realm` | String | `jwt` | Bearer `WWW-Authenticate` challenge 使用的 realm; [字符规则](README.md#authentication-realm) |
| `storeClaimsInCtx` | Boolean | `false` | 是否将已验证 claims 存储到 ctx 变量 `jwt_claims`（供下游插件通过 `get_ctx_var` 读取；该变量已从 access log 排除，claims 不会被持久化） |
| `claimsToHeaders` | Object | 无 | 将 JWT claim 名映射到上游请求头名称 |

**约束**：未设置 `anonymous` 时必须配置 `issuerGroups`。

### JwtIssuerGroup 子表

| 字段 | 类型 | 必需 | 说明 |
|------|------|------|------|
| `name` | String | 是 | 组名，字符集遵循 K8s label-value（`^[a-zA-Z0-9]([-_.a-zA-Z0-9]*[a-zA-Z0-9])?$`，≤63 字符）。成功时出现在访问日志 `group=<name>` |
| `description` | String | 否 | 人可读说明，不影响运行时 |
| `issuers` | Array | 是 | 该组覆盖的 issuer 列表（即 JWT `iss` 声明的值）。跨组唯一，不可重复 |
| `secretRefs` | Array | 是 | 该组使用的 Secret 引用列表（非空）。每个 Secret 需包含 `key` + `secret`（HS*）或 `publicKey`（RS*/ES*）|
| `algorithm` | String | 否 | 覆盖插件级 `algorithm`。未设置则继承插件级默认值 |
| `audiences` | Array | 否 | 覆盖插件级 `audiences`。设为 `[]` 表示该组明确不做 aud 检查 |

---

## 支持的算法

| 类型 | 算法 | 说明 |
|------|------|------|
| 对称 (HMAC) | `HS256`, `HS384`, `HS512` | 使用共享密钥签名，Secret 需包含 `secret` 字段 |
| 非对称 (RSA) | `RS256`, `RS384`, `RS512` | 使用 RSA 公钥验证，Secret 需包含 `publicKey` 字段（PEM 格式） |
| 非对称 (ECDSA) | `ES256`, `ES384` | 使用 ECDSA 公钥验证，Secret 需包含 `publicKey` 字段（PEM 格式） |

> **注意**：ES512（P-521）因底层库限制暂不支持。

---

## Token 携带方式

插件按以下优先级提取 Token：**Header > Query > Cookie**

### 1. Authorization Header（推荐）

```bash
curl -H "Authorization: Bearer eyJhbGciOiJIUzI1..." https://api.example.com/resource
```

### 2. Query 参数

> **默认禁用。** URL 中的 bearer token 会泄漏到 access log、浏览器历史和 `Referer` 头（RFC 6750 §2.3/§5.3）。要使用该方式必须显式设置 `allowTokenInQuery: true`；否则仅通过 query 携带的 token 会被忽略，请求按未认证处理。

```yaml
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        allowTokenInQuery: true   # 读取 query 中的 token 时必须开启
        # ...
```

```bash
curl "https://api.example.com/resource?jwt=eyJhbGciOiJIUzI1..."
```

### 3. Cookie

```bash
curl --cookie "jwt=eyJhbGciOiJIUzI1..." https://api.example.com/resource
```

---

## 按 issuer 组织：issuerGroups

`issuerGroups` 是 JwtAuth 的核心设计：每个 JWT 的 `iss` 声明决定使用哪个组的密钥和算法进行验证，彻底隔离不同颁发者的凭证。

### 单颁发者配置

```yaml
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        algorithm: HS256
        issuerGroups:
          - name: my-idp
            issuers:
              - "https://my-idp.example.com"
            secretRefs:
              - name: my-idp-secret
```

### 多颁发者配置（partner-a + partner-b）

适用于多个合作伙伴使用不同密钥签发 JWT 的场景：

```yaml
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        algorithm: HS256   # 插件级默认，被各组覆盖
        issuerGroups:
          - name: partner-a
            issuers:
              - "https://partner-a.example.com"
            secretRefs:
              - name: partner-a-secret
            # 使用插件级默认 algorithm: HS256
          - name: partner-b
            issuers:
              - "https://partner-b.example.com"
            secretRefs:
              - name: partner-b-secret
            algorithm: HS256   # 可显式指定，覆盖插件级默认
            audiences:
              - "my-api.example.com"   # 该组独立的受众检查
```

**路由逻辑**：
1. 网关先解码 JWT payload（不验签）提取 `iss` 声明
2. 在 `issuerGroups` 中查找匹配该 `iss` 的组
3. 检查 `header.alg` 必须等于组的 `algorithm`（防算法混淆攻击）
4. 用该组的密钥和算法进行完整验证

**组级 `algorithm` / `audiences` 覆盖语义**：
- `algorithm`：未设置时继承插件级默认；设置后覆盖插件级
- `audiences`：未设置时继承插件级默认；设为 `[]` 表示明确关闭该组的 aud 检查

**声明校验语义**：
- `exp` 必须存在，并使用 `lifetimeGracePeriod` 校验
- `nbf` 可选；存在时使用 `lifetimeGracePeriod` 校验
- 生效的 `audiences` 非空时，Token 必须包含 `aud`，且至少有一个值匹配
- 组级 `None` 会继承插件级值，不会关闭非空的插件级默认值。组级 `[]` 会显式覆盖任何插件级默认值并关闭校验。完成合并后，只有生效值为 `None`（两级都未设置）或空列表时才关闭校验

### 访问日志格式

验证成功时，访问日志记录：

```
OK group=partner-a iss=https://partner-a.example.com u=user-123
```

其中：
- `group=<name>`：路由到的 issuer 组名
- `iss=<iss>`：JWT 的颁发者声明值
- `u=<sub>`：JWT 的 `sub`（subject）声明值；该值按敏感日志数据处理，并由配置的策略进行掩码；`sub` 缺失或不是字符串时记录为 `(missing sub)`

### 失败分支的安全说明

**验证失败时，访问日志不包含 `group=` / `iss=` 信息**（设计决定）。

原因：未认证的攻击者不应能通过错误响应枚举已配置的 issuer、推断各颁发者使用的算法，或探测各组的 payload 凭证 key 分布——这些信息组合起来可绘制出多租户拓扑图，存在安全风险。

失败分支示例：

- `Deny malformed token` — Token 格式错误
- `Deny missing iss` — Token 缺少 `iss` 声明
- `Deny unknown iss` — `iss` 不匹配任何已配置的组
- `Deny bad algorithm` — Token 头部 `alg` 与组算法不匹配
- `Deny missing key` — `keyClaimName` 指定的 payload 选择器声明缺失、不是字符串，或没有匹配路由组内的凭证 key
- `Deny invalid signature` — 签名验证失败
- `Deny token expired` / `Deny immature token` / `Deny invalid audience` — 声明验证失败

所有认证失败都返回相同的通用 401 响应体。这些静态 access log 分类仅供运维诊断，不包含 Token、issuer、group、算法或密钥标识。

---

## 常见配置场景

### 1. HS256 单密钥

**创建 Secret**：
```yaml
apiVersion: v1
kind: Secret
metadata:
  name: jwt-secret
type: Opaque
stringData:
  key: "default-key"
  secret: "my-super-secret-key-at-least-32-chars!"
```

**配置插件**：
```yaml
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        algorithm: HS256
        hideCredentials: true
        issuerGroups:
          - name: default
            issuers:
              - "https://my-idp.example.com"
            secretRefs:
              - name: jwt-secret
```

### 2. RSA 公钥模式（RS256）

**创建 Secret（包含 PEM 格式公钥）**：
```yaml
apiVersion: v1
kind: Secret
metadata:
  name: jwt-rsa-public
type: Opaque
stringData:
  key: "rsa-key"
  publicKey: |
    -----BEGIN PUBLIC KEY-----
    MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA...
    -----END PUBLIC KEY-----
```

**配置插件**：
```yaml
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        algorithm: RS256
        issuerGroups:
          - name: rsa-idp
            issuers:
              - "https://rsa-idp.example.com"
            secretRefs:
              - name: jwt-rsa-public
```

### 3. 匿名访问模式

允许未认证的请求通过，但会标记为匿名用户：

```yaml
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        algorithm: HS256
        anonymous: "guest"
        issuerGroups:
          - name: default
            issuers:
              - "https://my-idp.example.com"
            secretRefs:
              - name: jwt-secret
```

**行为**：
- 有效 Token：按正常流程验证。除非通过 `claimsToHeaders` 显式映射声明，否则不会添加消费者请求头
- 未提供 Token：设置 `X-Consumer-Username: guest` 和 `X-Anonymous-Consumer: true`
- 提供无效 Token：返回与其他认证失败相同的通用 401，不使用匿名回退

---

## Secret 数据格式

每个 Secret 需包含以下字段：

| 算法类型 | 必需字段 | 说明 |
|----------|----------|------|
| HS* | `key` | 标识符，与 JWT payload 中 `keyClaimName` 对应字段匹配 |
| HS* | `secret` | HMAC 共享密钥（建议 32 字节以上） |
| RS*/ES* | `key` | 标识符，与 JWT payload 中 `keyClaimName` 对应字段匹配 |
| RS*/ES* | `publicKey` | PEM 格式公钥 |

凭证选择仅使用 `keyClaimName` 指定的 JWT payload 声明（默认为 `key`）。JOSE header 中的 `kid` 或 `keyid` 不参与选钥。

Kubernetes 总会先解码 Secret `.data` 的外层 Base64，之后 Controller 才读取 UTF-8 `secret` 值。`base64Secret: false` 直接把这些 UTF-8 字节作为 HMAC key。`base64Secret: true` 则要求该 UTF-8 字符串本身是 standard Base64，并再次解码得到实际的 HMAC key 字节。

---

## 响应头说明

### 认证成功

JwtAuth 默认不添加消费者请求头。要转发 `sub`，请显式配置声明映射，例如 `claimsToHeaders: {sub: X-Consumer-Username}`。映射后的请求头值受大小限制，并替换配置的目标请求头。

### 认证失败

返回 **401 Unauthorized**：

```
HTTP/1.1 401 Unauthorized
Content-Type: text/plain

401 Unauthorized - Invalid or missing JWT
```

- 缺少 Token：`WWW-Authenticate: Bearer realm="<realm>"`。
- Token 存在但无效：使用相同 realm，并添加 `error="invalid_token"` 和通用 `error_description="Unauthorized - Invalid or missing JWT"`。

---

## 客户端使用示例

### JavaScript (生成 JWT)

```javascript
// 使用 jsonwebtoken 库
const jwt = require('jsonwebtoken');

const token = jwt.sign(
  { sub: 'my-user', iss: 'https://my-idp.example.com', key: 'default-key', exp: Math.floor(Date.now() / 1000) + 3600 },
  'my-super-secret-key-at-least-32-chars!',
  { algorithm: 'HS256' }
);

fetch('https://api.example.com/resource', {
  headers: { 'Authorization': `Bearer ${token}` }
});
```

### Python

```python
import jwt
import time

token = jwt.encode(
    {
        'sub': 'my-user',
        'iss': 'https://my-idp.example.com',
        'key': 'default-key',
        'exp': int(time.time()) + 3600
    },
    'my-super-secret-key-at-least-32-chars!',
    algorithm='HS256'
)

import requests
response = requests.get(
    'https://api.example.com/resource',
    headers={'Authorization': f'Bearer {token}'}
)
```

### cURL

```bash
# 假设已有 Token
TOKEN="eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9..."

# Header 方式
curl -H "Authorization: Bearer $TOKEN" https://api.example.com/resource

# Query 方式
curl "https://api.example.com/resource?jwt=$TOKEN"
```

---

## 安全最佳实践

### 推荐做法

1. **始终使用 HTTPS**：JWT 在传输中不应被拦截

2. **为每个颁发者创建独立的 issuerGroup**：隔离密钥，防止跨颁发者密钥混用

3. **设置合理的过期时间**：
   ```json
   {"sub": "user", "iss": "https://my-idp.example.com", "key": "default-key", "exp": 1735689600}
   ```

4. **隐藏凭证**：
   ```yaml
   hideCredentials: true  # 不传递 Token 给上游
   ```

5. **使用非对称算法**：生产环境建议使用 RS256 或 ES256，私钥仅签发方持有

6. **配置 audiences**：指定允许的受众列表，防止 JWT 被其他服务复用

### 避免做法

1. **不要在 URL 中传递 Token** —— query 方式默认关闭（`allowTokenInQuery: false`），仅在确实无法使用 Header/Cookie 时才开启
2. **不要使用过短的密钥**（HS256 建议 32 字节以上）
3. **不要忽略 exp 声明**
4. **不要让多个颁发者共享同一个 issuerGroup**（应各自独立）

---

## 故障排除

### 问题 1：始终返回 401

**可能原因**：
- Secret 不存在或名称错误
- Secret 缺少必需字段（`secret` 或 `publicKey`）
- 算法与密钥类型不匹配
- JWT 的 `iss` 声明与任何 issuerGroup 的 `issuers` 不匹配

**解决方案**：
```bash
kubectl get secret jwt-secret -o yaml
# 确保包含 key + secret 或 key + publicKey 字段
```

### 问题 2：Token 验证失败

**可能原因**：
- 密钥不匹配
- Token 已过期（检查 `exp` 声明）
- 算法不匹配（Token 中声明的 alg 与 issuerGroup 配置不一致）
- JWT 缺少 `iss` 声明
- JWT payload 缺少 `keyClaimName` 配置的声明（默认为 `key`）；header 中的 `kid` 会被忽略

**解决方案**：
```bash
# 解码 Token 检查内容（不验证签名）
echo "eyJhbG..." | cut -d. -f2 | base64 -d
```

### 问题 3：多颁发者模式找不到对应组

**可能原因**：
- JWT payload 中 `iss` 值与任何 issuerGroup 的 `issuers` 列表不匹配（包括尾部斜杠差异）
- `issuers` 配置了重复的 iss 值（跨组 iss 必须唯一）

**解决方案**：
确保 JWT payload 中的 `iss` 与 issuerGroups 配置完全匹配（字符串精确匹配，含协议头和路径）。

---

## 完整示例

```yaml
# 1. 创建 Secret（partner-a）
apiVersion: v1
kind: Secret
metadata:
  name: partner-a-secret
  namespace: default
type: Opaque
stringData:
  key: "partner-a-key"
  secret: "partner-a-secret-key-32-chars-long!!"
---
# 2. 创建 Secret（partner-b）
apiVersion: v1
kind: Secret
metadata:
  name: partner-b-secret
  namespace: default
type: Opaque
stringData:
  key: "partner-b-key"
  secret: "partner-b-secret-key-32-chars-long!!"
---
# 3. 创建 EdgionPlugins
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: jwt-auth-plugin
  namespace: default
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        algorithm: HS256
        header: authorization
        hideCredentials: true
        issuerGroups:
          - name: partner-a
            issuers:
              - "https://partner-a.example.com"
            secretRefs:
              - name: partner-a-secret
          - name: partner-b
            issuers:
              - "https://partner-b.example.com"
            secretRefs:
              - name: partner-b-secret
            audiences:
              - "my-api.example.com"
---
# 4. 创建 HTTPRoute
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: protected-api
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
  hostnames:
    - "api.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: jwt-auth-plugin
      backendRefs:
        - name: backend-service
          port: 8080
```

**测试**：
```bash
# 生成 partner-a 的 Token（示例使用 Node.js）
TOKEN=$(node -e "console.log(require('jsonwebtoken').sign({sub:'user1',iss:'https://partner-a.example.com',key:'partner-a-key',exp:Math.floor(Date.now()/1000)+3600},'partner-a-secret-key-32-chars-long!!',{algorithm:'HS256'}))")

# 使用 Token 访问（访问日志将记录 OK group=partner-a iss=https://partner-a.example.com u=user1）
curl -H "Authorization: Bearer $TOKEN" https://api.example.com/api/data

# 无 Token - 返回 401
curl https://api.example.com/api/data
```

JwtAuth 使用 Controller 解析后的 Kubernetes Secret，不获取 JWKS，也不执行 OIDC discovery。需要 OIDC/JWKS provider 行为时，请使用 OpenID Connect 插件。

---

## 相关链接

- [BasicAuth](./basic-auth.md) — 基本认证
- [KeyAuth](./key-auth.md) — API Key 认证
- [HmacAuth](./hmac-auth.md) — HMAC 认证

### Cookie 凭证清理

启用 `hideCredentials: true` 后，Cookie 鉴权成功会删除所有 Cookie 头字段中同名的凭证。
其他 Cookie 的名称和值保留，但分隔符可能规范化，多个字段可能合并。
后续插件、Webhook 和上游请求读取修改后的头。清理遇到非 UTF-8 字节时返回 HTTP 400，
内部更新失败时返回 HTTP 500；两种失败均不会继续转发未清理的请求。
没有 Cookie 修改时，不会重新组装原始 Cookie 字段。
