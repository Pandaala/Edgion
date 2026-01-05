# CORS Plugin User Guide

## 什么是 CORS？

CORS (Cross-Origin Resource Sharing，跨域资源共享) 是一种浏览器安全机制，用于控制哪些网站可以访问你的 API。

**简单例子**：
- 你的 API 在 `https://api.example.com`
- 你的前端在 `https://app.example.com`
- 没有 CORS 配置，浏览器会阻止前端访问 API
- 配置 CORS 后，浏览器允许跨域访问

## 快速开始

### 最简单的配置（开发环境）

```yaml
filters:
  - type: Cors
    config:
      allow_origins: "https://app.example.com"
      allow_methods: "GET,POST,PUT,DELETE"
      allow_headers: "Content-Type,Authorization"
```

### 默认值说明

为了安全，CORS 插件采用**拒绝所有**的默认策略，你必须显式配置允许的来源。

如果不配置 `allow_origins`，所有跨域请求都会被拒绝。

---

## 配置参数

### 基础参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `allow_origins` | String | `""` (空) | **必填**。允许的来源域名，多个用逗号分隔。示例：`"https://app.com,https://admin.com"` |
| `allow_origins_by_regex` | Array | 无 | 使用正则表达式匹配来源。示例：`["^https://.*\\.example\\.com$"]` |
| `allow_methods` | String | `"GET,HEAD,OPTIONS"` | 允许的 HTTP 方法，多个用逗号分隔。常用：`"GET,POST,PUT,DELETE,PATCH"` |
| `allow_headers` | String | `"Accept,Accept-Language,Content-Language,Content-Type,Range"` | 允许的请求头，多个用逗号分隔。常用：`"Content-Type,Authorization"` |
| `expose_headers` | String | `""` (空) | 允许浏览器访问的响应头。示例：`"X-Request-ID,X-Total-Count"` |

### 安全参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `allow_credentials` | Boolean | `false` | 是否允许发送 Cookie 和认证信息。设为 `true` 时不能使用通配符 `*` |
| `max_age` | Integer | 无 | 预检请求的缓存时间（秒）。推荐：`86400` (24小时) |

### 高级参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `preflight_continue` | Boolean | `false` | 是否将预检请求转发给上游服务。通常保持 `false` |
| `allow_private_network` | Boolean | `false` | 启用 Private Network Access (Chrome 94+)。用于从公网访问私网资源 |
| `timing_allow_origins` | String | 无 | 允许访问 Resource Timing API 的来源。格式同 `allow_origins` |
| `timing_allow_origins_by_regex` | Array | 无 | 使用正则表达式匹配 Timing API 来源 |

### 特殊值说明

| 值 | 含义 | 使用场景 |
|----|------|----------|
| `*` | 通配符，允许所有 | **不推荐生产使用**。仅用于开发环境 |
| `**` | 强制通配符，绕过安全检查 | **危险**。仅在完全理解风险时使用 |
| `""` (空字符串) | 拒绝所有 | 默认值，安全 |

---

## 常见配置场景

### 1. 开发环境：允许所有来源

⚠️ **仅用于开发！不要在生产环境使用！**

```yaml
filters:
  - type: Cors
    config:
      allow_origins: "*"
      allow_methods: "*"
      allow_headers: "*"
```

### 2. 生产环境：单个前端域名

```yaml
filters:
  - type: Cors
    config:
      allow_origins: "https://app.example.com"
      allow_methods: "GET,POST,PUT,DELETE"
      allow_headers: "Content-Type,Authorization"
      expose_headers: "X-Request-ID"
      max_age: 86400
```

### 3. 多个前端域名

```yaml
filters:
  - type: Cors
    config:
      allow_origins: "https://app.example.com,https://admin.example.com,https://mobile.example.com"
      allow_methods: "GET,POST,PUT,DELETE"
      allow_headers: "Content-Type,Authorization"
      max_age: 86400
```

### 4. 允许所有子域名

使用通配符匹配：

```yaml
filters:
  - type: Cors
    config:
      allow_origins: "*.example.com"
      allow_methods: "GET,POST,PUT,DELETE"
      allow_headers: "Content-Type,Authorization"
```

或使用正则表达式（更灵活）：

```yaml
filters:
  - type: Cors
    config:
      allow_origins: "https://example.com"  # 主域名
      allow_origins_by_regex:
        - "^https://.*\\.example\\.com$"    # 所有子域名
      allow_methods: "GET,POST,PUT,DELETE"
      allow_headers: "Content-Type,Authorization"
```

### 5. 允许本地开发（localhost）

```yaml
filters:
  - type: Cors
    config:
      allow_origins: "https://app.example.com"
      allow_origins_by_regex:
        - "^http://localhost:[0-9]+$"        # localhost:任意端口
        - "^http://127\\.0\\.0\\.1:[0-9]+$"  # 127.0.0.1:任意端口
      allow_methods: "GET,POST,PUT,DELETE"
      allow_headers: "Content-Type,Authorization"
```

### 6. 带身份认证（Cookie/Token）

⚠️ **重要**：使用 `allow_credentials: true` 时，**不能**使用通配符 `*`

```yaml
filters:
  - type: Cors
    config:
      allow_origins: "https://app.example.com"  # 必须是具体域名
      allow_methods: "GET,POST,PUT,DELETE"
      allow_headers: "Content-Type,Authorization,X-Custom-Token"
      allow_credentials: true                    # 允许发送 Cookie
      max_age: 86400
```

### 7. 限制只读访问

```yaml
filters:
  - type: Cors
    config:
      allow_origins: "https://public.example.com"
      allow_methods: "GET,HEAD,OPTIONS"  # 只允许读取操作
      allow_headers: "Accept,Content-Type"
```

### 8. RESTful API 完整配置

```yaml
filters:
  - type: Cors
    config:
      # 允许的来源
      allow_origins: "https://app.example.com,https://admin.example.com"
      
      # RESTful 方法
      allow_methods: "GET,POST,PUT,DELETE,PATCH,OPTIONS"
      
      # 常用请求头
      allow_headers: "Content-Type,Authorization,X-Request-ID,X-Api-Key"
      
      # 暴露自定义响应头
      expose_headers: "X-Request-ID,X-Total-Count,X-Page-Count"
      
      # 启用身份认证
      allow_credentials: true
      
      # 缓存预检结果 24 小时
      max_age: 86400
```

### 9. 性能监控场景（Timing API）

如果需要允许第三方监控服务访问性能数据：

```yaml
filters:
  - type: Cors
    config:
      allow_origins: "https://app.example.com"
      allow_methods: "GET,POST"
      allow_headers: "Content-Type"
      
      # 允许监控服务访问 Resource Timing API
      timing_allow_origins: "https://analytics.example.com"
```

### 10. Private Network Access（访问内网）

Chrome 94+ 从公网访问内网资源需要额外配置：

```yaml
filters:
  - type: Cors
    config:
      allow_origins: "https://app.example.com"
      allow_methods: "GET,POST"
      allow_headers: "Content-Type"
      allow_private_network: true  # 启用 Private Network Access
```

---

## 常见问题

### Q1: 为什么配置了 CORS 还是报错？

**A**: 检查以下几点：

1. **Origin 拼写正确**：包括协议（http/https）、域名、端口
   ```yaml
   ✅ 正确: "https://app.example.com"
   ❌ 错误: "app.example.com" (缺少协议)
   ❌ 错误: "https://app.example.com/" (多了尾斜杠)
   ```

2. **端口号匹配**：
   ```yaml
   ✅ "http://localhost:3000"  # 指定端口
   ✅ "^http://localhost:[0-9]+$"  # 正则匹配任意端口
   ```

3. **使用 credentials 时不能用通配符**：
   ```yaml
   ❌ 错误配置:
   allow_origins: "*"
   allow_credentials: true
   
   ✅ 正确配置:
   allow_origins: "https://app.example.com"
   allow_credentials: true
   ```

### Q2: 开发环境可以用 `*`，生产环境怎么办？

**A**: 使用环境变量或不同的配置文件：

```yaml
# development.yaml
filters:
  - type: Cors
    config:
      allow_origins: "*"
      allow_methods: "*"
      allow_headers: "*"

# production.yaml
filters:
  - type: Cors
    config:
      allow_origins: "https://app.example.com"
      allow_methods: "GET,POST,PUT,DELETE"
      allow_headers: "Content-Type,Authorization"
```

### Q3: 如何调试 CORS 问题？

**A**: 查看浏览器开发者工具的 Console 和 Network 面板：

1. **Console** 会显示具体的 CORS 错误信息
2. **Network** 面板查看请求头和响应头：
   - 请求头：`Origin`, `Access-Control-Request-Method`, `Access-Control-Request-Headers`
   - 响应头：`Access-Control-Allow-Origin`, `Access-Control-Allow-Methods`, `Access-Control-Allow-Headers`

### Q4: 什么是预检请求（Preflight）？

**A**: 对于某些"复杂"请求，浏览器会先发送一个 OPTIONS 请求询问服务器是否允许：

**触发预检的情况**：
- 使用 `PUT`, `DELETE`, `PATCH` 等方法
- 使用自定义请求头（如 `X-Custom-Header`）
- `Content-Type` 为 `application/json`

**不触发预检的情况**（简单请求）：
- 使用 `GET`, `HEAD`, `POST` 方法
- 只使用基本请求头
- `Content-Type` 为 `application/x-www-form-urlencoded`, `multipart/form-data`, `text/plain`

### Q5: `max_age` 设多少合适？

**A**: 推荐值：

```yaml
开发环境: max_age: 600       # 10 分钟，方便测试
测试环境: max_age: 3600      # 1 小时
生产环境: max_age: 86400     # 24 小时，减少预检请求
```

### Q6: 正则表达式怎么写？

**A**: 常用正则示例：

```yaml
# 所有子域名
"^https://.*\\.example\\.com$"

# localhost 任意端口
"^http://localhost:[0-9]+$"

# 多个顶级域名
"^https://app\\.(com|net|org)$"

# 开发环境域名
"^https://.*\\.dev\\.example\\.com$"

# IP 地址范围
"^http://192\\.168\\.1\\.[0-9]{1,3}:[0-9]+$"
```

**注意**：正则表达式中的 `.` 需要转义为 `\\.`

---

## 安全最佳实践

### ✅ 推荐做法

1. **最小权限原则**：只允许必需的来源、方法和头
   ```yaml
   allow_origins: "https://app.example.com"  # 具体域名
   allow_methods: "GET,POST"                 # 只允许需要的方法
   allow_headers: "Content-Type"             # 只允许需要的头
   ```

2. **生产环境不用通配符**：
   ```yaml
   ❌ allow_origins: "*"
   ✅ allow_origins: "https://app.example.com"
   ```

3. **使用 credentials 时必须指定具体域名**：
   ```yaml
   ✅ 安全:
   allow_origins: "https://app.example.com"
   allow_credentials: true
   ```

4. **设置合理的 max_age**：
   ```yaml
   max_age: 86400  # 24 小时，平衡性能和灵活性
   ```

5. **只暴露必要的响应头**：
   ```yaml
   expose_headers: "X-Request-ID"  # 只暴露需要的头
   ```

### ❌ 避免的做法

1. **生产环境使用 `*`**：
   ```yaml
   ❌ 危险:
   allow_origins: "*"
   allow_methods: "*"
   allow_headers: "*"
   ```

2. **credentials 配合通配符**：
   ```yaml
   ❌ 无效配置（会报错）:
   allow_origins: "*"
   allow_credentials: true
   ```

3. **不必要的方法**：
   ```yaml
   ❌ 过度开放:
   allow_methods: "GET,POST,PUT,DELETE,PATCH,OPTIONS,TRACE,CONNECT"
   
   ✅ 按需开放:
   allow_methods: "GET,POST,PUT,DELETE"
   ```

---

## 性能优化建议

1. **使用 max_age 缓存预检请求**：
   ```yaml
   max_age: 86400  # 浏览器缓存预检结果 24 小时
   ```

2. **避免过多的正则表达式**：
   ```yaml
   ❌ 性能差:
   allow_origins_by_regex:
     - "^https://app1\\.example\\.com$"
     - "^https://app2\\.example\\.com$"
     - "^https://app3\\.example\\.com$"
   
   ✅ 性能好:
   allow_origins: "https://app1.example.com,https://app2.example.com,https://app3.example.com"
   ```

3. **子域名通配符优先用 `*.example.com`**：
   ```yaml
   ✅ 快速:
   allow_origins: "*.example.com"
   
   ⚠️ 较慢:
   allow_origins_by_regex:
     - "^https://.*\\.example\\.com$"
   ```

---

## 完整配置示例

### 典型的生产环境配置

```yaml
apiVersion: gateway.edgion.io/v1
kind: HTTPRoute
metadata:
  name: api-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - "api.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/
      filters:
        - type: Cors
          config:
            # 允许的前端域名
            allow_origins: "https://app.example.com,https://admin.example.com"
            
            # RESTful API 方法
            allow_methods: "GET,POST,PUT,DELETE,PATCH,OPTIONS"
            
            # 允许的请求头
            allow_headers: "Content-Type,Authorization,X-Request-ID,X-Api-Version"
            
            # 暴露的响应头（供前端读取）
            expose_headers: "X-Request-ID,X-Total-Count,X-Rate-Limit-Remaining"
            
            # 允许发送 Cookie 和 Token
            allow_credentials: true
            
            # 预检请求缓存 24 小时
            max_age: 86400
      backendRefs:
        - name: api-service
          port: 8080
```

---

## 相关链接

- [MDN: CORS 详细说明](https://developer.mozilla.org/en-US/docs/Web/HTTP/CORS)
- [WHATWG Fetch Standard](https://fetch.spec.whatwg.org/)
- Edgion 网关文档：[插件系统](./plugins.md)

---

## 获取帮助

如果遇到问题：

1. 检查浏览器控制台的错误信息
2. 查看网关日志：`logs/access.log` 和 `logs/edgion-gateway.log`
3. 参考本文档的常见问题部分
4. 提交 Issue 到 GitHub

**记住**：CORS 是浏览器的安全机制，服务端配置正确后，由浏览器负责验证和执行。

