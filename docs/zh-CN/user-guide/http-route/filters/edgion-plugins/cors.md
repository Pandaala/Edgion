# CORS 插件

> **🔌 Edgion 扩展**
> 
> CORS 是 `EdgionPlugins` CRD 提供的跨域配置插件，不属于标准 Gateway API。

## 什么是 CORS？

**CORS（Cross-Origin Resource Sharing，跨域资源共享）** 是一种浏览器安全机制，用于控制某个来源的网页能否访问另一个来源的资源。

**来源（origin）** 是协议 + 域名 + 端口的组合。例如，`https://app.example.com` 和 `https://api.example.com` 是不同的来源（不同子域名）。

当前端应用向不同来源的 API 发起请求时，浏览器会发送 CORS 预检请求或附加 `Origin` 头，服务器必须返回正确的 CORS 响应头才能授权访问。

## 快速开始

### 最简配置

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: cors-plugin
  namespace: production
spec:
  requestPlugins:
    - type: Cors
      config:
        allowOrigins: "https://app.example.com"
        allowMethods: "GET,POST,PUT,DELETE"
        allowHeaders: "Content-Type,Authorization"
```

### 默认值说明

默认值以安全为优先：

| 字段 | 默认值 | 含义 |
|------|--------|------|
| `allowOrigins` | `""` (空) | **拒绝所有来源**——必须显式配置 |
| `allowMethods` | `GET,HEAD,OPTIONS` | 仅允许安全方法 |
| `allowHeaders` | `Accept,Accept-Language,Content-Language,Content-Type,Range` | CORS 安全列表请求头 |
| `allowCredentials` | `false` | 不允许携带凭据 |
| `maxAge` | 无 | 不缓存预检请求 |

---

## 配置参数

### 基础参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `allowOrigins` | String | `""` (空) | 允许的来源，多个用逗号分隔。支持 `*`、`**` 和 `*.domain`。示例：`"https://app.example.com,https://admin.example.com"` |
| `allowMethods` | String | `GET,HEAD,OPTIONS` | 允许的 HTTP 方法，多个用逗号分隔。支持 `*` / `**` |
| `allowHeaders` | String | `Accept,Accept-Language,Content-Language,Content-Type,Range` | 允许的请求头，多个用逗号分隔。支持 `*` / `**` |
| `exposeHeaders` | String | `""` (空) | 向浏览器暴露的响应头，多个用逗号分隔 |
| `allowCredentials` | Boolean | `false` | 是否允许携带凭据（Cookie、Authorization 头） |
| `maxAge` | Integer | 无 | 预检请求缓存时长（秒）。未设置时不发送 `Access-Control-Max-Age` 头 |

### 高级参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `allowOriginsByRegex` | Array\<String\> | 无 | 来源匹配的正则表达式列表。用于允许 `http://` 来源（`*.domain` 简写仅适用于 HTTPS） |
| `preflightContinue` | Boolean | `false` | 是否将 OPTIONS 预检请求转发给上游，而不是由网关直接响应 |
| `allowPrivateNetwork` | Boolean | `false` | 启用 Private Network Access 支持（Chrome 94+） |
| `timingAllowOrigins` | String | 无 | `Timing-Allow-Origin` 响应头的值。支持 `*` / `**` |
| `timingAllowOriginsByRegex` | Array\<String\> | 无 | `Timing-Allow-Origin` 来源匹配的正则表达式列表 |

### 特殊值说明

`allowOrigins` 支持多种特殊形式：

| 值 | 含义 | 使用场景 |
|----|------|----------|
| `*` | 通配符，允许所有来源 | **生产环境不推荐** |
| `**` | 强制通配符，允许所有来源 | **请避免使用** |
| `*.example.com` | 子域名通配符，匹配 `https://<任意>.example.com`（仅 HTTPS；**不**匹配根域名或 `http://`） | 多子域名应用 |
| `""` (空) | 拒绝所有来源 | 默认值，安全 |

⚠️ **通配符来源（`*` / `**`）与 `allowCredentials: true` 组合将在解析时被拒绝。** 同样的限制也适用于 `allowMethods`、`allowHeaders`、`exposeHeaders` 和 `timingAllowOrigins`：当 `allowCredentials` 为 `true` 时，均不能包含 `*`。这与浏览器规则一致——携带凭据的响应不能使用通配符。

`null` 来源始终被拒绝，即使 `allowOrigins` 为 `*`。

---

## 常见配置场景

### 1. 单一前端来源

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: cors-single
  namespace: production
spec:
  requestPlugins:
    - type: Cors
      config:
        allowOrigins: "https://app.example.com"
        allowMethods: "GET,POST"
        allowHeaders: "Content-Type"
```

### 2. 多个指定来源并启用凭据

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: cors-multi
  namespace: production
spec:
  requestPlugins:
    - type: Cors
      config:
        allowOrigins: "https://app.example.com,https://admin.example.com"
        allowMethods: "GET,POST,PUT,DELETE"
        allowHeaders: "Content-Type,Authorization"
        allowCredentials: true
```

### 3. 子域名通配符

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: cors-subdomain
  namespace: production
spec:
  requestPlugins:
    - type: Cors
      config:
        # 匹配 https://<任意>.example.com，仅 HTTPS
        allowOrigins: "*.example.com"
        allowMethods: "GET,POST"
        allowCredentials: true
```

### 4. 公共 API，带预检缓存（无凭据）

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: cors-public
  namespace: production
spec:
  requestPlugins:
    - type: Cors
      config:
        allowOrigins: "*"
        allowMethods: "GET"
        allowHeaders: "Content-Type"
        # 预检结果缓存 1 小时
        maxAge: 3600
```

### 5. 暴露并反射响应头

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: cors-headers
  namespace: production
spec:
  requestPlugins:
    - type: Cors
      config:
        allowOrigins: "https://app.example.com"
        allowMethods: "GET,POST"
        # 反射浏览器请求的任意请求头
        allowHeaders: "**"
        # 允许 JavaScript 读取这些响应头
        exposeHeaders: "X-Total-Count,X-Page-Number"
```

### 6. 完整生产配置

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: cors-full
  namespace: production
spec:
  requestPlugins:
    - type: Cors
      config:
        allowOrigins: "https://app.example.com,https://admin.example.com"
        allowMethods: "GET,POST,PUT,DELETE,PATCH"
        allowHeaders: "Content-Type,Authorization,X-Request-ID"
        exposeHeaders: "X-Total-Count,X-Page-Number"
        allowCredentials: true
        # 预检请求缓存 24 小时
        maxAge: 86400
        allowPrivateNetwork: false
```

---

## 常见问题

### Q1：为什么配置了 CORS 还是被浏览器拦截？

**A**：按以下顺序逐一排查：

1. 是否配置了 `allowOrigins`？默认值为空（拒绝所有）。
2. 请求的 `Origin` 是否精确匹配（协议 + 域名 + 端口）？
3. 携带凭据的请求是否设置了 `allowCredentials: true`？
4. 所需的请求头是否在 `allowHeaders` 中列出？

### Q2：为什么通配符来源与凭据组合会失败？

**A**：这是设计如此。当 `allowCredentials: true` 时，`allowOrigins`（以及 `allowMethods`、`allowHeaders`、`exposeHeaders`、`timingAllowOrigins`）不能使用 `*` 或 `**`，插件会在解析时拒绝此类配置。请改用显式来源或 `*.domain` 子域名通配符。

### Q3：如何为本地开发允许 `http://` 来源？

**A**：`*.domain` 简写仅适用于 HTTPS。对于 `http://` 来源，请使用 `allowOriginsByRegex`：

```yaml
config:
  allowOriginsByRegex:
    - "^http://localhost:[0-9]+$"
```

### Q4：`*.example.com` 是否匹配根域名？

**A**：不匹配。`*.example.com` 匹配 `https://api.example.com`，但不匹配 `https://example.com`。如果需要根域名，请在 `allowOrigins` 中显式列出：`allowOrigins: "https://example.com,*.example.com"`。

---

## 安全最佳实践

### ✅ 推荐做法

```yaml
✅ 正确：
# 显式来源
allowOrigins: "https://app.example.com"
# 指定方法
allowMethods: "GET,POST"
# 指定请求头
allowHeaders: "Content-Type,Authorization"
# 仅在需要时启用凭据
allowCredentials: true
```

### ❌ 需要避免

```yaml
❌ 错误：
# 生产环境使用通配符来源
allowOrigins: "*"
# 通配符与凭据组合（解析时将被拒绝）
allowOrigins: "*"
allowCredentials: true
```

---

## 完整配置示例

```yaml
apiVersion: gateway.networking.k8s.io/v1
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
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: cors-full
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: cors-full
  namespace: production
spec:
  requestPlugins:
    - type: Cors
      config:
        allowOrigins: "https://app.example.com,https://admin.example.com"
        allowMethods: "GET,POST,PUT,DELETE,PATCH"
        allowHeaders: "Content-Type,Authorization,X-Request-ID"
        exposeHeaders: "X-Total-Count,X-Page-Number"
        allowCredentials: true
        maxAge: 86400
        allowPrivateNetwork: false
```

## 相关链接

- [CSRF 插件](./csrf.md)
- [过滤器目录](../README.md)
- [MDN: CORS](https://developer.mozilla.org/en-US/docs/Web/HTTP/CORS)
- [W3C: Private Network Access](https://wicg.github.io/private-network-access/)
