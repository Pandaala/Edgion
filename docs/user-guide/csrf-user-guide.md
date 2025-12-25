# CSRF Plugin User Guide

## 什么是 CSRF？

CSRF (Cross-Site Request Forgery，跨站请求伪造) 是一种攻击方式，攻击者诱导用户在已登录的网站上执行非预期的操作。

**攻击示例**：
1. 用户登录了银行网站 `bank.com`
2. 用户访问恶意网站 `evil.com`
3. `evil.com` 发送请求到 `bank.com/transfer?to=attacker&amount=1000`
4. 由于用户已登录，银行执行了转账操作

**CSRF 插件的保护机制**：
- 为每个用户生成唯一的随机 token
- 安全方法（GET/HEAD/OPTIONS）：自动设置 token cookie
- 不安全方法（POST/PUT/DELETE）：验证请求中的 token 是否匹配

## 快速开始

### 最简单的配置

```yaml
filters:
  - type: Csrf
    config:
      key: "your-32-char-secret-key-here!!"
```

### 工作流程

1. **客户端首次访问（GET）**：
   ```bash
   curl https://api.example.com/api/data
   # 响应包含: Set-Cookie: apisix-csrf-token=<token>
   ```

2. **客户端提交表单（POST）**：
   ```bash
   curl -X POST https://api.example.com/api/submit \
     -H "apisix-csrf-token: <token>" \
     -H "Cookie: apisix-csrf-token=<token>" \
     -d "data=value"
   # 请求成功
   ```

---

## 配置参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `key` | String | 无 | **必填**。用于签名 token 的密钥。建议 32 字符以上，使用强随机值 |
| `expires` | Integer | `7200` (2小时) | Token 过期时间（秒）。建议与会话过期时间一致 |
| `name` | String | `"apisix-csrf-token"` | Token 名称，用于 cookie 和 header |

---

## 常见配置场景

### 1. 基础配置（推荐）

```yaml
filters:
  - type: Csrf
    config:
      key: "9Kx8mV2nP5qR7tY3zB6cF1gH4jL0wX"
      expires: 7200  # 2 hours
```

### 2. 自定义 Token 名称

```yaml
filters:
  - type: Csrf
    config:
      key: "your-secret-key"
      name: "x-csrf-token"  # 自定义名称
      expires: 3600  # 1 hour
```

### 3. 长期有效 Token

```yaml
filters:
  - type: Csrf
    config:
      key: "your-secret-key"
      expires: 86400  # 24 hours
```

⚠️ **注意**：expires 时间越长，token 被盗用的风险越高。

---

## 客户端集成

### HTML 表单

```html
<!DOCTYPE html>
<html>
<head>
  <title>Submit Form</title>
</head>
<body>
  <form id="myForm" action="https://api.example.com/api/submit" method="POST">
    <input type="text" name="username" />
    <button type="submit">Submit</button>
  </form>

  <script>
    // 从 cookie 中读取 token
    function getCookie(name) {
      const value = `; ${document.cookie}`;
      const parts = value.split(`; ${name}=`);
      if (parts.length === 2) return parts.pop().split(';').shift();
    }

    // 提交表单时添加 CSRF token 头
    document.getElementById('myForm').addEventListener('submit', function(e) {
      e.preventDefault();
      
      const token = getCookie('apisix-csrf-token');
      const formData = new FormData(this);
      
      fetch(this.action, {
        method: 'POST',
        headers: {
          'apisix-csrf-token': token  // 添加 token 到 header
        },
        body: formData,
        credentials: 'include'  // 发送 cookie
      }).then(response => {
        console.log('Success:', response);
      });
    });
  </script>
</body>
</html>
```

### JavaScript (Fetch API)

```javascript
// 1. 首次加载页面，获取 CSRF token
async function initCsrfToken() {
  await fetch('https://api.example.com/api/init', {
    credentials: 'include'  // 允许 cookie
  });
  // 服务器会在响应中设置 csrf token cookie
}

// 2. 从 cookie 读取 token
function getCsrfToken() {
  const value = `; ${document.cookie}`;
  const parts = value.split('; apisix-csrf-token=');
  if (parts.length === 2) {
    return parts.pop().split(';').shift();
  }
  return null;
}

// 3. 发送 POST 请求
async function submitData(data) {
  const token = getCsrfToken();
  
  const response = await fetch('https://api.example.com/api/submit', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'apisix-csrf-token': token  // 添加 token
    },
    body: JSON.stringify(data),
    credentials: 'include'  // 发送 cookie
  });
  
  return response.json();
}

// 使用示例
(async () => {
  await initCsrfToken();
  await submitData({ username: 'admin' });
})();
```

### React 示例

```javascript
import React, { useState, useEffect } from 'react';
import axios from 'axios';

// 配置 axios 自动发送 cookie
axios.defaults.withCredentials = true;

function App() {
  const [csrfToken, setCsrfToken] = useState('');

  useEffect(() => {
    // 获取 CSRF token
    axios.get('https://api.example.com/api/init').then(() => {
      const token = getCsrfToken();
      setCsrfToken(token);
      
      // 设置 axios 默认 header
      axios.defaults.headers.common['apisix-csrf-token'] = token;
    });
  }, []);

  const handleSubmit = async (e) => {
    e.preventDefault();
    
    // axios 会自动带上 cookie 和 header
    const response = await axios.post('/api/submit', {
      username: 'admin'
    });
    
    console.log('Success:', response.data);
  };

  return (
    <form onSubmit={handleSubmit}>
      <input type="text" name="username" />
      <button type="submit">Submit</button>
    </form>
  );
}

function getCsrfToken() {
  const value = `; ${document.cookie}`;
  const parts = value.split('; apisix-csrf-token=');
  return parts.length === 2 ? parts.pop().split(';').shift() : null;
}

export default App;
```

### Vue.js 示例

```javascript
import axios from 'axios';

// 全局配置
axios.defaults.withCredentials = true;

// 请求拦截器：自动添加 CSRF token
axios.interceptors.request.use(config => {
  const token = getCsrfToken();
  if (token) {
    config.headers['apisix-csrf-token'] = token;
  }
  return config;
});

function getCsrfToken() {
  const value = `; ${document.cookie}`;
  const parts = value.split('; apisix-csrf-token=');
  return parts.length === 2 ? parts.pop().split(';').shift() : null;
}

// 组件中使用
export default {
  methods: {
    async submitForm() {
      await axios.post('/api/submit', {
        username: this.username
      });
    }
  }
}
```

---

## 安全最佳实践

### ✅ 推荐做法

1. **使用强密钥**
   ```yaml
   # ✅ 好：32+ 字符，强随机
   key: "9Kx8mV2nP5qR7tY3zB6cF1gH4jL0wX8dR2fT9pN5mQ"
   
   # ❌ 差：短且可预测
   key: "secret123"
   ```

2. **生成随机密钥**
   ```bash
   # Linux/macOS
   openssl rand -base64 32
   
   # 或
   head -c 32 /dev/urandom | base64
   ```

3. **定期轮换密钥**
   - 建议每 30-90 天更换一次密钥
   - 更换后旧 token 会失效

4. **与会话管理配合**
   ```yaml
   expires: 7200  # 与会话过期时间一致
   ```

5. **使用 HTTPS**
   - CSRF token 通过 cookie 传输，必须使用 HTTPS
   - Cookie 设置了 `SameSite=Lax` 提供额外保护

### ❌ 避免做法

1. **不要使用弱密钥**
   ```yaml
   # ❌ 危险
   key: "123456"
   key: "password"
   key: "secret"
   ```

2. **不要在客户端暴露密钥**
   - 密钥只在服务端使用
   - 客户端只需要知道 token 值

3. **不要禁用 CSRF 保护**
   ```yaml
   # ❌ 不要为了方便而移除 CSRF 插件
   ```

---

## 请求方法处理

| 方法 | CSRF 检查 | 行为 |
|------|-----------|------|
| `GET` | ❌ 跳过 | 自动设置 token cookie |
| `HEAD` | ❌ 跳过 | 自动设置 token cookie |
| `OPTIONS` | ❌ 跳过 | 自动设置 token cookie |
| `POST` | ✅ 验证 | 必须提供有效 token |
| `PUT` | ✅ 验证 | 必须提供有效 token |
| `DELETE` | ✅ 验证 | 必须提供有效 token |
| `PATCH` | ✅ 验证 | 必须提供有效 token |

**说明**：
- 安全方法（GET/HEAD/OPTIONS）被认为是幂等的，不会修改数据
- 不安全方法（POST/PUT/DELETE/PATCH）会修改数据，需要 CSRF 保护

---

## 错误响应

### 错误 1：请求头中没有 token

```json
HTTP/1.1 401 Unauthorized
Content-Type: application/json

{
  "error_msg": "no csrf token in headers"
}
```

**原因**：POST 请求未包含 CSRF token header

**解决方案**：
```javascript
fetch('/api/submit', {
  method: 'POST',
  headers: {
    'apisix-csrf-token': token  // 添加此行
  }
});
```

### 错误 2：Cookie 中没有 token

```json
HTTP/1.1 401 Unauthorized

{
  "error_msg": "no csrf cookie"
}
```

**原因**：
- 首次访问未通过 GET 请求获取 token
- Cookie 被清除或过期

**解决方案**：先访问一个 GET 端点获取 token

### 错误 3：Token 不匹配

```json
HTTP/1.1 401 Unauthorized

{
  "error_msg": "csrf token mismatch"
}
```

**原因**：Header 中的 token 与 Cookie 中的不一致

**解决方案**：确保从 cookie 读取的 token 值用于 header

### 错误 4：Token 签名验证失败

```json
HTTP/1.1 401 Unauthorized

{
  "error_msg": "Failed to verify the csrf token signature"
}
```

**原因**：
- Token 被篡改
- Token 已过期
- 服务端密钥已更换

**解决方案**：重新获取新的 token

---

## 故障排除

### 问题 1：本地开发跨域问题

**症状**：Cookie 无法设置，提示 SameSite 错误

**原因**：
- 前端 `http://localhost:3000`
- 后端 `http://localhost:8080`
- 不同端口被视为跨域

**解决方案**：
```yaml
# 配置 CORS 允许 credentials
filters:
  - type: Cors
    config:
      allowOrigins: "http://localhost:3000"
      allowCredentials: true  # 允许 cookie
  
  - type: Csrf
    config:
      key: "your-secret-key"
```

### 问题 2：移动应用集成

**症状**：移动应用无法使用 cookie

**解决方案**：
- 方案 1：使用 WebView（支持 cookie）
- 方案 2：改用 JWT 认证（更适合移动端）

### 问题 3：Token 频繁过期

**原因**：expires 设置过短

**解决方案**：
```yaml
filters:
  - type: Csrf
    config:
      key: "your-secret-key"
      expires: 14400  # 延长到 4 小时
```

---

## 与其他插件配合

### 1. 与 CORS 配合

```yaml
filters:
  # 先处理 CORS
  - type: Cors
    config:
      allowOrigins: "https://app.example.com"
      allowCredentials: true  # 必须启用
      allowHeaders: "Content-Type,apisix-csrf-token"  # 允许 CSRF header
  
  # 再进行 CSRF 验证
  - type: Csrf
    config:
      key: "your-secret-key"
```

**重要**：
- `allowCredentials: true` 是必需的（cookie 需要）
- `allowHeaders` 必须包含 CSRF token 名称

### 2. 与 Basic Auth 配合

```yaml
filters:
  # 先进行认证
  - type: BasicAuth
    config:
      secretRefs:
        - name: api-user
  
  # 再进行 CSRF 验证
  - type: Csrf
    config:
      key: "your-secret-key"
```

---

## 完整示例

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: web-api
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
            name: security-plugins
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: security-plugins
  namespace: default
spec:
  plugins:
    - enable: true
      plugin:
        type: Cors
        config:
          allowOrigins: "https://app.example.com"
          allowMethods: "GET,POST,PUT,DELETE"
          allowHeaders: "Content-Type,Authorization,apisix-csrf-token"
          allowCredentials: true
          maxAge: 86400
    
    - enable: true
      plugin:
        type: Csrf
        config:
          key: "9Kx8mV2nP5qR7tY3zB6cF1gH4jL0wX8d"
          expires: 7200
          name: "apisix-csrf-token"
```

**测试**：
```bash
# 1. 获取 CSRF token (GET 请求)
curl -c cookies.txt https://api.example.com/api/users

# 2. 使用 token 提交数据 (POST 请求)
TOKEN=$(grep apisix-csrf-token cookies.txt | awk '{print $7}')
curl -X POST https://api.example.com/api/submit \
  -b cookies.txt \
  -H "apisix-csrf-token: $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"data":"value"}'
```

