# ResponseRewrite 插件

> **🔌 Edgion 扩展**
> 
> ResponseRewrite 是 `EdgionPlugins` CRD 提供的响应重写插件，不属于标准 Gateway API。

## 什么是 ResponseRewrite？

ResponseRewrite 在将响应返回给客户端之前，对响应进行重写，包括：

- **状态码修改**：修改 HTTP 响应状态码
- **响应头操作**：
  - **set**：设置响应头（覆盖已存在的）
  - **add**：添加响应头（追加到已存在的）
  - **remove**：删除响应头
  - **rename**：重命名响应头（其他网关少有的功能）

## 快速开始

### 最简单的配置

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-response-rewrite
spec:
  upstreamResponseFilterPlugins:
    - type: ResponseRewrite
      config:
        statusCode: 200
```

---

## 配置参数

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `statusCode` | Integer | 否 | HTTP 状态码 (100-599) |
| `headers` | Object | 否 | 响应头修改操作 |
| `headers.set` | Array | 否 | 设置响应头（覆盖已有值） |
| `headers.add` | Array | 否 | 添加响应头（追加到已有值） |
| `headers.remove` | Array | 否 | 删除响应头 |
| `headers.rename` | Array | 否 | 重命名响应头 |

### Headers 配置格式

**set/add 格式**：
```yaml
headers:
  set:
    - name: "Header-Name"
      value: "Header-Value"
  add:
    - name: "Header-Name"
      value: "Header-Value"
```

**remove 格式**：
```yaml
headers:
  remove:
    - "Header-Name-1"
    - "Header-Name-2"
```

**rename 格式**：
```yaml
headers:
  rename:
    - from: "Old-Header-Name"
      to: "New-Header-Name"
```

---

## 配置场景

### 1. 修改状态码

将上游返回的状态码统一修改：

```yaml
config:
  statusCode: 200
```

**效果**：无论上游返回什么状态码，客户端都收到 `200 OK`

### 2. 设置响应头

设置或覆盖响应头：

```yaml
config:
  headers:
    set:
      - name: Cache-Control
        value: "no-cache, no-store"
      - name: X-Content-Type-Options
        value: "nosniff"
```

**效果**：设置 `Cache-Control` 和 `X-Content-Type-Options` 响应头

### 3. 添加响应头

追加新的响应头（不覆盖已有值）：

```yaml
config:
  headers:
    add:
      - name: X-Powered-By
        value: "Edgion"
      - name: X-Response-Time
        value: "50ms"
```

**效果**：添加 `X-Powered-By` 和 `X-Response-Time` 响应头

### 4. 删除响应头

移除敏感或不需要的响应头：

```yaml
config:
  headers:
    remove:
      - Server
      - X-Powered-By
      - X-AspNet-Version
```

**效果**：删除 `Server`、`X-Powered-By`、`X-AspNet-Version` 响应头

### 5. 重命名响应头

将内部响应头重命名为对外暴露的名称：

```yaml
config:
  headers:
    rename:
      - from: X-Internal-Request-Id
        to: X-Request-Id
      - from: X-Backend-Server
        to: X-Upstream-Server
```

**效果**：`X-Internal-Request-Id` 重命名为 `X-Request-Id`

### 6. 综合配置

结合多种重写功能：

```yaml
config:
  statusCode: 200
  headers:
    rename:
      - from: X-Internal-Id
        to: X-Request-Id
    set:
      - name: Cache-Control
        value: "no-cache"
      - name: X-API-Version
        value: "v2"
    add:
      - name: X-Powered-By
        value: "Edgion"
    remove:
      - Server
      - X-Debug
```

---

## 执行顺序

当同时配置多个操作时，执行顺序为：

1. **状态码修改**（statusCode）
2. **响应头重命名**（rename）- 先重命名，保证后续操作使用新名称
3. **响应头添加**（add）
4. **响应头设置**（set）
5. **响应头删除**（remove）- 最后删除，确保不会删除刚添加的头

---

## 注意事项

### 1. 状态码范围

状态码必须在 100-599 范围内：

```yaml
# ✅ 正确
config:
  statusCode: 201

# ❌ 错误 - 超出范围
config:
  statusCode: 600
```

### 2. rename 的工作方式

`rename` 会将原响应头的值复制到新名称，然后删除原响应头：

```yaml
config:
  headers:
    rename:
      - from: X-Old
        to: X-New
```

**效果**：如果响应包含 `X-Old: value123`，则变为 `X-New: value123`

**注意**：如果原响应头不存在，`rename` 操作会被跳过，不会产生错误。

### 3. 响应头名称大小写

HTTP 响应头名称是大小写不敏感的，但建议使用标准的大小写格式（如 `Content-Type` 而不是 `content-type`）。

---

## 完整示例

### HTTPRoute + EdgionPlugins 配置

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
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
            name: api-response-rewrite
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: api-response-rewrite
  namespace: default
spec:
  upstreamResponseFilterPlugins:
    - type: ResponseRewrite
      config:
        # 统一返回 200
        statusCode: 200
        headers:
          # 重命名内部头
          rename:
            - from: X-Internal-Request-Id
              to: X-Request-Id
          # 设置缓存和安全头
          set:
            - name: Cache-Control
              value: "no-cache, no-store"
            - name: X-Content-Type-Options
              value: "nosniff"
            - name: X-Frame-Options
              value: "DENY"
          # 添加标识
          add:
            - name: X-Powered-By
              value: "Edgion"
          # 删除敏感头
          remove:
            - Server
            - X-AspNet-Version
            - X-Debug
```

### 测试

```bash
# 请求
curl -i "https://api.example.com/api/users"

# 响应（重写后）：
# HTTP/1.1 200 OK
# X-Request-Id: abc123          (从 X-Internal-Request-Id 重命名)
# Cache-Control: no-cache, no-store
# X-Content-Type-Options: nosniff
# X-Frame-Options: DENY
# X-Powered-By: Edgion
# (Server 头已删除)
# (X-Debug 头已删除)
```

---

## 与其他插件配合

### 与 ProxyRewrite 配合

请求阶段用 ProxyRewrite 重写请求，响应阶段用 ResponseRewrite 重写响应：

```yaml
spec:
  requestPlugins:
    - type: ProxyRewrite
      config:
        uri: "/internal$uri"
        host: "backend.internal.svc"
  upstreamResponseFilterPlugins:
    - type: ResponseRewrite
      config:
        headers:
          remove:
            - Server
          add:
            - name: X-Gateway
              value: "Edgion"
```

### 与 Cors 配合

ResponseRewrite 可以补充 CORS 插件的响应头：

```yaml
spec:
  requestPlugins:
    - type: Cors
      config:
        allow_origins: "*"
        allow_methods: "GET,POST,PUT,DELETE"
  upstreamResponseFilterPlugins:
    - type: ResponseRewrite
      config:
        headers:
          set:
            - name: Access-Control-Max-Age
              value: "86400"
```

---

## 与其他网关对比

| 特性 | Edgion ResponseRewrite | APISIX response-rewrite | Kong response-transformer |
|------|------------------------|-------------------------|---------------------------|
| 状态码修改 | ✅ | ✅ | ❌ |
| 响应头 set | ✅ | ✅ | ✅ (replace) |
| 响应头 add | ✅ | ✅ | ✅ |
| 响应头 remove | ✅ | ✅ | ✅ |
| 响应头 rename | ✅ | ❌ | ✅ |
| Body 修改 | ❌ | ✅ | ✅ (JSON) |
| 条件匹配 | ❌ (第二阶段) | ✅ | ❌ |

---

## 故障排除

### 问题 1：响应头未被修改

**检查**：
1. 确认 EdgionPlugins 已正确关联到 HTTPRoute
2. 确认插件类型为 `ResponseRewrite` 而不是 `ProxyRewrite`
3. 确认响应头名称拼写正确

### 问题 2：rename 不生效

**检查**：
1. 确认原响应头在上游响应中存在
2. 检查 `from` 字段的名称是否正确（大小写）

### 问题 3：配置验证失败

**常见原因**：
1. `statusCode` 超出 100-599 范围
2. `headers` 中的 `name` 或 `from`/`to` 为空

**解决**：查看网关启动日志中的配置验证错误信息。

---

## 性能说明

- **同步执行**：ResponseRewrite 在 `upstream_response_filter` 阶段同步执行，不涉及异步操作
- **低延迟**：仅修改响应头元数据，不涉及响应体处理
- **内存高效**：不缓冲响应体，直接流式传输
