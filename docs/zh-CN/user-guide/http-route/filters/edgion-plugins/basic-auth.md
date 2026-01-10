# Basic Auth 插件

> **🔌 Edgion 扩展**
> 
> BasicAuth 是 `EdgionPlugins` CRD 提供的认证插件，不属于标准 Gateway API。

## 什么是 Basic Auth？

Basic Auth（基本认证）是一种简单的 HTTP 认证机制，要求客户端在请求头中提供用户名和密码。

**工作原理**：
1. 客户端发送带有 `Authorization: Basic base64(username:password)` 头的请求
2. 插件验证用户名和密码
3. 验证成功：允许访问，设置 `X-Consumer-Username` 头传递给上游
4. 验证失败：返回 401 状态码，要求认证

## 快速开始

### 最简单的配置

```yaml
filters:
  - type: BasicAuth
    config:
      secretRefs:
        - name: my-users-secret
      realm: "API Gateway"
```

### 创建 Kubernetes Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: my-users-secret
  namespace: default
type: kubernetes.io/basic-auth
stringData:
  username: "admin"
  password: "secret123"
```

**多个用户**：创建多个 Secret，并在 `secretRefs` 中引用：

```yaml
filters:
  - type: BasicAuth
    config:
      secretRefs:
        - name: admin-user
        - name: api-user
        - name: readonly-user
```

---

## 配置参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `secretRefs` | Array | 无 | **必填**。Kubernetes Secret 引用列表。每个 Secret 必须是 `kubernetes.io/basic-auth` 类型，包含 `username` 和 `password` 字段 |
| `realm` | String | `"API Gateway"` | 认证域名称，显示在浏览器登录框中 |
| `hideCredentials` | Boolean | `false` | 是否隐藏 Authorization 头，不传递给上游服务 |
| `anonymous` | String | 无 | 匿名用户名。设置后，未认证请求也会被允许，并设置此用户名 |

---

## 常见配置场景

### 1. 基础配置：单个用户

**创建 Secret**：
```yaml
apiVersion: v1
kind: Secret
metadata:
  name: api-user
type: kubernetes.io/basic-auth
stringData:
  username: "apiuser"
  password: "mySecretPassword123"
```

**配置插件**：
```yaml
filters:
  - type: BasicAuth
    config:
      secretRefs:
        - name: api-user
      hideCredentials: true  # 不传递密码给上游
```

### 2. 多用户配置

**创建多个 Secret**：
```yaml
---
apiVersion: v1
kind: Secret
metadata:
  name: admin-user
type: kubernetes.io/basic-auth
stringData:
  username: "admin"
  password: "adminPass123"
---
apiVersion: v1
kind: Secret
metadata:
  name: developer-user
type: kubernetes.io/basic-auth
stringData:
  username: "developer"
  password: "devPass456"
---
apiVersion: v1
kind: Secret
metadata:
  name: readonly-user
type: kubernetes.io/basic-auth
stringData:
  username: "readonly"
  password: "readPass789"
```

**配置插件**：
```yaml
filters:
  - type: BasicAuth
    config:
      secretRefs:
        - name: admin-user
        - name: developer-user
        - name: readonly-user
      realm: "My API"
      hideCredentials: true
```

### 3. 匿名访问模式

允许未认证的请求通过，但会标记为匿名用户：

```yaml
filters:
  - type: BasicAuth
    config:
      secretRefs:
        - name: premium-user
      anonymous: "guest"  # 未认证用户标记为 "guest"
```

**行为**：
- 提供正确凭证：设置 `X-Consumer-Username: premium-user`
- 未提供凭证：设置 `X-Consumer-Username: guest` 和 `X-Anonymous-Consumer: true`

### 4. 自定义认证域

```yaml
filters:
  - type: BasicAuth
    config:
      secretRefs:
        - name: api-user
      realm: "Protected API - Please Login"  # 浏览器显示的提示
```

---

## 客户端使用示例

### cURL

```bash
# 方式 1：使用 -u 参数
curl -u username:password https://api.example.com/resource

# 方式 2：手动构造 Authorization 头
curl -H "Authorization: Basic $(echo -n 'username:password' | base64)" \
  https://api.example.com/resource
```

### JavaScript (Fetch API)

```javascript
const username = 'myuser';
const password = 'mypass';
const credentials = btoa(`${username}:${password}`);

fetch('https://api.example.com/resource', {
  headers: {
    'Authorization': `Basic ${credentials}`
  }
});
```

### Python (requests)

```python
import requests

response = requests.get(
    'https://api.example.com/resource',
    auth=('username', 'password')
)
```

---

## 响应头说明

### 认证成功

插件会自动设置以下请求头传递给上游：

| 头名称 | 说明 | 示例 |
|--------|------|------|
| `X-Consumer-Username` | 认证成功的用户名 | `admin` |
| `X-Anonymous-Consumer` | 是否为匿名用户 | `true` (仅匿名模式) |

### 认证失败

返回 **401 Unauthorized**，带有以下响应头：

```
HTTP/1.1 401 Unauthorized
WWW-Authenticate: Basic realm="API Gateway"
Content-Type: text/plain

401 Unauthorized - Authentication required
```

---

## 安全最佳实践

### ✅ 推荐做法

1. **始终使用 HTTPS**
   ```yaml
   # Basic Auth 传输明文凭证（base64 编码），必须使用 HTTPS
   ```

2. **使用强密码**
   - 最少 12 个字符
   - 包含大小写字母、数字和特殊字符
   - 不使用常见密码

3. **隐藏凭证**
   ```yaml
   hideCredentials: true  # 不传递 Authorization 头给上游
   ```

4. **定期更换密码**
   ```bash
   # 更新 Secret
   kubectl create secret generic api-user \
     --from-literal=username=apiuser \
     --from-literal=password=newPassword123 \
     --dry-run=client -o yaml | kubectl apply -f -
   ```

5. **最小权限原则**
   - 为不同权限创建不同用户
   - 只读用户使用只读凭证

### ❌ 避免做法

1. **不要在 HTTP 上使用 Basic Auth**
   ```yaml
   # ❌ 危险：凭证会被明文传输
   # ✅ 必须使用 HTTPS
   ```

2. **不要在代码中硬编码密码**
   ```yaml
   # ❌ 不要这样做
   # password: "hardcoded_password"
   
   # ✅ 使用 Kubernetes Secret
   secretRefs:
     - name: user-secret
   ```

3. **不要对公开 API 使用 Basic Auth**
   - Basic Auth 适合内部 API 或管理界面
   - 公开 API 应使用 OAuth 2.0 或 JWT

---

## 故障排除

### 问题 1：始终返回 401

**原因**：
- Secret 不存在或名称错误
- Secret 类型不是 `kubernetes.io/basic-auth`
- Secret 不包含 `username` 和 `password` 字段

**解决方案**：
```bash
# 检查 Secret
kubectl get secret api-user -o yaml

# 确保类型正确
type: kubernetes.io/basic-auth

# 确保包含必需字段
data:
  username: ...
  password: ...
```

### 问题 2：密码验证失败

**原因**：
- 密码不匹配
- 密码使用了 base64 编码（应使用原始密码）

**解决方案**：
```yaml
# ✅ 正确：使用 stringData（自动编码）
stringData:
  username: "admin"
  password: "myPassword123"

# ❌ 错误：手动 base64 编码
data:
  username: YWRtaW4=
  password: bXlQYXNzd29yZDEyMw==  # 会被再次编码
```

### 问题 3：上游收到 Authorization 头

**原因**：未设置 `hideCredentials: true`

**解决方案**：
```yaml
filters:
  - type: BasicAuth
    config:
      secretRefs:
        - name: api-user
      hideCredentials: true  # 添加此行
```

---

## 与其他插件配合

### 1. 与 CORS 配合使用

```yaml
filters:
  # 先处理 CORS
  - type: Cors
    config:
      allowOrigins: "https://app.example.com"
      allowCredentials: true  # Basic Auth 需要此项
      allowHeaders: "Content-Type,Authorization"  # 允许 Authorization 头
  
  # 再进行认证
  - type: BasicAuth
    config:
      secretRefs:
        - name: api-user
```

**注意**：CORS 插件必须允许 `Authorization` 头。

### 2. 与 IP Restriction 配合

```yaml
filters:
  # 先限制 IP
  - type: IpRestriction
    config:
      allow: ["10.0.0.0/8", "192.168.0.0/16"]
  
  # 再进行认证
  - type: BasicAuth
    config:
      secretRefs:
        - name: internal-api-user
```

**优势**：双重保护，只有内网 IP + 正确凭证才能访问。

---

## 性能考虑

- **密码哈希**：插件使用 bcrypt 哈希密码（启动时计算），运行时验证性能良好
- **并发请求**：每个请求都会进行密码验证，对于高并发场景建议配合缓存使用
- **Secret 更新**：修改 Secret 后，插件会自动重新加载（可能有短暂延迟）

---

## 限制

1. **仅支持 Kubernetes Secret**
   - 不支持文件或环境变量配置
   - 必须使用 `kubernetes.io/basic-auth` 类型

2. **不支持动态权限**
   - 所有用户拥有相同的访问权限
   - 如需细粒度权限控制，请使用上游服务实现

3. **不支持用户管理 API**
   - 需要通过 Kubernetes API 管理用户
   - 不提供用户注册、密码重置等功能

---

## 完整示例

### Gateway API HTTPRoute 配置

```yaml
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
            name: auth-edgion_plugins
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: auth-edgion_plugins
  namespace: default
spec:
  plugins:
    - enable: true
      plugin:
        type: BasicAuth
        config:
          secretRefs:
            - name: api-admin
            - name: api-developer
          realm: "Protected API"
          hideCredentials: true
---
apiVersion: v1
kind: Secret
metadata:
  name: api-admin
type: kubernetes.io/basic-auth
stringData:
  username: "admin"
  password: "AdminSecret123!"
---
apiVersion: v1
kind: Secret
metadata:
  name: api-developer
type: kubernetes.io/basic-auth
stringData:
  username: "developer"
  password: "DevSecret456!"
```

**测试**：
```bash
# 使用 admin 用户
curl -u admin:AdminSecret123! https://api.example.com/api/users

# 使用 developer 用户
curl -u developer:DevSecret456! https://api.example.com/api/data

# 无凭证 - 返回 401
curl https://api.example.com/api/users
```

