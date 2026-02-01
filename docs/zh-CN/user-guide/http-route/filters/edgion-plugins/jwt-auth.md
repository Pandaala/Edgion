# JWT Auth 插件

> **🔌 Edgion 扩展**
> 
> JwtAuth 是 `EdgionPlugins` CRD 提供的认证插件，不属于标准 Gateway API。

## 什么是 JWT Auth？

JWT Auth（JSON Web Token 认证）是一种基于令牌的认证机制，客户端在请求中携带 JWT，由网关验证签名和声明后放行。

**工作原理**：
1. 客户端发送带有 JWT 的请求（通过 Header、Query 或 Cookie）
2. 插件验证 JWT 签名、过期时间等声明
3. 验证成功：允许访问，设置 `X-Consumer-Username` 头传递给上游
4. 验证失败：返回 401 状态码

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
        secretRef:
          name: jwt-secret
        algorithm: HS256
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
  secret: "my-jwt-secret-key-32-chars-long!!"
```

---

## 配置参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `secretRef` | Object | 无 | 单密钥模式：指向包含 `secret`（HS*）或 `publicKey`（RS*/ES*）的 Secret |
| `secretRefs` | Array | 无 | 多密钥模式：每个 Secret 需包含 `key`（标识符）+ `secret` 或 `publicKey` |
| `algorithm` | String | `HS256` | 签名算法，见下方支持的算法列表 |
| `header` | String | `authorization` | 从哪个 Header 读取 Token（支持 `Bearer <token>` 或裸 token） |
| `query` | String | `jwt` | Query 参数名 |
| `cookie` | String | `jwt` | Cookie 名称 |
| `hideCredentials` | Boolean | `false` | 是否隐藏 Token，不传递给上游服务 |
| `anonymous` | String | 无 | 匿名用户名。设置后，未认证请求也会被允许，并设置此用户名 |
| `keyClaimName` | String | `key` | JWT payload 中用于选择密钥的 claim 名称（多密钥模式） |
| `lifetimeGracePeriod` | Integer | `0` | exp/nbf 时钟偏差容忍（秒） |

**约束**：必须配置 `secretRef` 或 `secretRefs` 之一。

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

```bash
curl "https://api.example.com/resource?jwt=eyJhbGciOiJIUzI1..."
```

### 3. Cookie

```bash
curl --cookie "jwt=eyJhbGciOiJIUzI1..." https://api.example.com/resource
```

---

## 常见配置场景

### 1. 单密钥模式（HS256）

**创建 Secret**：
```yaml
apiVersion: v1
kind: Secret
metadata:
  name: jwt-secret
type: Opaque
stringData:
  secret: "my-super-secret-key-at-least-32-chars!"
```

**配置插件**：
```yaml
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        secretRef:
          name: jwt-secret
        algorithm: HS256
        hideCredentials: true
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
        secretRef:
          name: jwt-rsa-public
        algorithm: RS256
```

### 3. 多密钥模式

适用于多个服务/颁发者使用不同密钥签发 JWT 的场景。

**创建多个 Secret**：
```yaml
---
apiVersion: v1
kind: Secret
metadata:
  name: issuer-a-secret
type: Opaque
stringData:
  key: "issuer-a"           # 用于匹配 JWT payload 中的 key claim
  secret: "issuer-a-secret-key-32-chars-long!!"
---
apiVersion: v1
kind: Secret
metadata:
  name: issuer-b-secret
type: Opaque
stringData:
  key: "issuer-b"
  secret: "issuer-b-secret-key-32-chars-long!!"
```

**配置插件**：
```yaml
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        secretRefs:
          - name: issuer-a-secret
          - name: issuer-b-secret
        algorithm: HS256
        keyClaimName: key    # JWT payload 中必须包含 {"key": "issuer-a"} 或 {"key": "issuer-b"}
```

### 4. 匿名访问模式

允许未认证的请求通过，但会标记为匿名用户：

```yaml
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        secretRef:
          name: jwt-secret
        anonymous: "guest"
```

**行为**：
- 提供有效 Token：设置 `X-Consumer-Username: <key_claim_value>`
- 未提供 Token 或 Token 无效：设置 `X-Consumer-Username: guest` 和 `X-Anonymous-Consumer: true`

---

## Secret 数据格式

### 单密钥（secretRef）

| 算法类型 | 必需字段 | 说明 |
|----------|----------|------|
| HS* | `secret` | HMAC 共享密钥（建议 32 字节以上） |
| RS*/ES* | `publicKey` | PEM 格式公钥 |

### 多密钥（secretRefs）

| 字段 | 说明 |
|------|------|
| `key` | 标识符，与 JWT payload 中 `keyClaimName` 对应字段匹配 |
| `secret` | HS* 算法的共享密钥 |
| `publicKey` | RS*/ES* 算法的 PEM 格式公钥 |

---

## 响应头说明

### 认证成功

| 头名称 | 说明 | 示例 |
|--------|------|------|
| `X-Consumer-Username` | JWT payload 中 `keyClaimName` 对应的值 | `user-123` |

### 认证失败

返回 **401 Unauthorized**：

```
HTTP/1.1 401 Unauthorized
Content-Type: text/plain

401 Unauthorized - Invalid or missing JWT
```

---

## 客户端使用示例

### JavaScript (生成 JWT)

```javascript
// 使用 jsonwebtoken 库
const jwt = require('jsonwebtoken');

const token = jwt.sign(
  { key: 'my-user', exp: Math.floor(Date.now() / 1000) + 3600 },
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
    {'key': 'my-user', 'exp': int(time.time()) + 3600},
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

2. **设置合理的过期时间**：
   ```json
   {"key": "user", "exp": 1735689600}  // exp 为 UNIX 时间戳
   ```

3. **隐藏凭证**：
   ```yaml
   hideCredentials: true  # 不传递 Token 给上游
   ```

4. **使用非对称算法**：生产环境建议使用 RS256 或 ES256，私钥仅签发方持有

5. **定期轮换密钥**：配合多密钥模式实现平滑轮换

### 避免做法

1. **不要在 URL 中传递 Token**（除非无法使用 Header/Cookie）
2. **不要使用过短的密钥**（HS256 建议 32 字节以上）
3. **不要忽略 exp 声明**

---

## 故障排除

### 问题 1：始终返回 401

**可能原因**：
- Secret 不存在或名称错误
- Secret 缺少必需字段（`secret` 或 `publicKey`）
- 算法与密钥类型不匹配

**解决方案**：
```bash
kubectl get secret jwt-secret -o yaml
# 确保包含 secret 或 publicKey 字段
```

### 问题 2：Token 验证失败

**可能原因**：
- 密钥不匹配
- Token 已过期（检查 `exp` 声明）
- 算法不匹配（Token 中声明的 alg 与配置不一致）

**解决方案**：
```bash
# 解码 Token 检查内容（不验证签名）
echo "eyJhbG..." | cut -d. -f2 | base64 -d
```

### 问题 3：多密钥模式找不到密钥

**可能原因**：
- JWT payload 中缺少 `keyClaimName` 对应的字段
- `keyClaimName` 的值与 Secret 中的 `key` 不匹配

**解决方案**：
确保 JWT payload 包含：
```json
{"key": "issuer-a", "exp": 1735689600}
```
且 Secret 中 `key` 字段值为 `issuer-a`。

---

## 完整示例

```yaml
# 1. 创建 Secret
apiVersion: v1
kind: Secret
metadata:
  name: api-jwt-secret
  namespace: default
type: Opaque
stringData:
  secret: "my-super-secret-key-for-jwt-auth-32!"
---
# 2. 创建 EdgionPlugins
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: jwt-auth-plugin
  namespace: default
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        secretRef:
          name: api-jwt-secret
        algorithm: HS256
        header: authorization
        hideCredentials: true
---
# 3. 创建 HTTPRoute
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
# 生成 Token（示例使用 Node.js）
TOKEN=$(node -e "console.log(require('jsonwebtoken').sign({key:'user1',exp:Math.floor(Date.now()/1000)+3600},'my-super-secret-key-for-jwt-auth-32!'))")

# 使用 Token 访问
curl -H "Authorization: Bearer $TOKEN" https://api.example.com/api/data

# 无 Token - 返回 401
curl https://api.example.com/api/data
```
