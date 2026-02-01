# ProxyRewrite 插件

> **🔌 Edgion 扩展**
> 
> ProxyRewrite 是 `EdgionPlugins` CRD 提供的请求重写插件，不属于标准 Gateway API。

## 什么是 ProxyRewrite？

ProxyRewrite 在将请求转发给上游服务之前，对请求进行重写，包括：

- **URI 重写**：修改请求路径
- **Host 重写**：修改 Host 请求头
- **Method 重写**：修改 HTTP 方法
- **Headers 修改**：添加、设置或删除请求头

## 快速开始

### 最简单的配置

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-proxy-rewrite
spec:
  requestPlugins:
    - enable: true
      type: ProxyRewrite
      config:
        uri: "/internal/api/v2"
```

---

## 配置参数

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `uri` | String | 否 | 新的请求路径，支持变量替换。与 `regexUri` 二选一，`uri` 优先级更高 |
| `regexUri` | Object | 否 | 正则表达式 URI 重写 |
| `regexUri.pattern` | String | 是 | 正则匹配模式 |
| `regexUri.replacement` | String | 是 | 替换模板，支持 `$1-$9` 捕获组 |
| `host` | String | 否 | 新的 Host 请求头值 |
| `method` | String | 否 | 新的 HTTP 方法：GET/POST/PUT/DELETE/PATCH/HEAD/OPTIONS 等 |
| `headers` | Object | 否 | 请求头修改操作 |
| `headers.add` | Array | 否 | 添加请求头（追加到已有值） |
| `headers.set` | Array | 否 | 设置请求头（覆盖已有值） |
| `headers.remove` | Array | 否 | 删除请求头 |

### 变量说明

模板中可使用以下变量：

| 变量 | 说明 | 示例 |
|------|------|------|
| `$uri` | 原始请求路径 | `/api/users` |
| `$arg_xxx` | 查询参数值 | `$arg_id` → 获取 `?id=123` 中的 `123` |
| `$1-$9` | 正则捕获组（仅 `regexUri` 场景） | `/users/$1` |
| `$xxx` | 路径参数（从 HTTPRoute 定义的 `/:xxx` 提取） | `$uid` |

**注意**：查询参数（Query String）会自动保留，无需手动处理。

---

## 配置场景

### 1. 简单 URI 重写

将所有请求重定向到固定路径：

```yaml
config:
  uri: "/internal/api/v2"
```

**效果**：`/api/users` → `/internal/api/v2`

### 2. 使用 $uri 变量

保留原始路径并添加前缀/后缀：

```yaml
config:
  uri: "/prefix$uri/suffix"
```

**效果**：`/api/users` → `/prefix/api/users/suffix`

### 3. 使用查询参数变量

从查询参数中提取值构建新路径：

```yaml
config:
  uri: "/search/$arg_keyword/$arg_lang"
```

**效果**：`/search?keyword=hello&lang=en` → `/search/hello/en`

### 4. 正则表达式重写

使用正则匹配和捕获组：

```yaml
config:
  regexUri:
    pattern: "^/api/v1/users/(\\d+)/profile"
    replacement: "/user-service/$1"
```

**效果**：`/api/v1/users/123/profile` → `/user-service/123`

### 5. 多捕获组

```yaml
config:
  regexUri:
    pattern: "^/api/(\\w+)/(\\d+)"
    replacement: "/internal/$1/id/$2"
```

**效果**：`/api/users/456` → `/internal/users/id/456`

### 6. Host 重写

修改请求的 Host 头：

```yaml
config:
  host: "backend.internal.svc"
```

### 7. Method 重写

将 GET 请求转换为 POST：

```yaml
config:
  method: "POST"
```

### 8. Headers 添加

追加新的请求头（不覆盖已有值）：

```yaml
config:
  headers:
    add:
      - name: X-Gateway
        value: "edgion"
      - name: X-Request-Source
        value: "external"
```

### 9. Headers 设置

设置请求头（覆盖已有值）：

```yaml
config:
  headers:
    set:
      - name: X-Api-Version
        value: "v2"
      - name: X-Original-Path
        value: "$uri"
```

### 10. Headers 删除

移除指定的请求头：

```yaml
config:
  headers:
    remove:
      - X-Debug
      - X-Internal-Token
```

### 11. 路径参数提取

当 HTTPRoute 使用路径参数模式时：

**HTTPRoute 配置**：
```yaml
rules:
  - matches:
      - path:
          type: PathPrefix
          value: /api/:uid/profile
```

**ProxyRewrite 配置**：
```yaml
config:
  uri: "/user-service/$uid/data"
  headers:
    set:
      - name: X-User-Id
        value: "$uid"
```

**效果**：`/api/123/profile` → `/user-service/123/data`，同时设置 `X-User-Id: 123`

### 12. 综合配置

结合多种重写功能：

```yaml
config:
  uri: "/internal$uri"
  host: "backend.internal.svc"
  method: "POST"
  headers:
    add:
      - name: X-Gateway
        value: "edgion"
    set:
      - name: X-Original-Path
        value: "$uri"
      - name: X-Request-Id
        value: "req-12345"
    remove:
      - X-Debug
```

---

## 执行顺序

当同时配置多个重写操作时，执行顺序为：

1. **URI 重写**（`uri` 或 `regexUri`）
2. **Host 重写**
3. **Method 重写**
4. **Headers 修改**（add → set → remove）

---

## 注意事项

### 1. URI 与 regexUri 优先级

当同时配置 `uri` 和 `regexUri` 时，**`uri` 优先级更高**，`regexUri` 会被忽略。

```yaml
# uri 会生效，regexUri 被忽略
config:
  uri: "/new/path"
  regexUri:
    pattern: "^/api/(.*)"
    replacement: "/internal/$1"
```

### 2. Host 字段冲突

不要同时在 `host` 字段和 `headers.set` 中设置 Host 头，会导致配置验证失败：

```yaml
# ❌ 错误配置
config:
  host: "backend.svc"
  headers:
    set:
      - name: Host
        value: "other.svc"

# ✅ 正确配置
config:
  host: "backend.svc"
```

### 3. 查询参数自动保留

URI 重写后，原始请求的查询参数会自动追加：

```yaml
config:
  uri: "/new/path"
```

**效果**：`/old/path?foo=bar&baz=qux` → `/new/path?foo=bar&baz=qux`

### 4. 变量 URL 编码

当 `$arg_xxx` 变量用于 URI 路径时，特殊字符会自动进行 URL 编码（RFC 3986）：

```yaml
config:
  uri: "/search/$arg_keyword"
```

**效果**：`/search?keyword=hello world` → `/search/hello%20world`

### 5. 未匹配的路径参数

如果 `$name` 变量在路由中未定义，变量会保持原样不替换：

```yaml
config:
  uri: "/api/$unknown/data"
```

**效果**：`/test` → `/api/$unknown/data`（变量未替换）

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
            value: /api/v1
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: api-rewrite
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: api-rewrite
  namespace: default
spec:
  requestPlugins:
    - enable: true
      type: ProxyRewrite
      config:
        # URI 重写：v1 → v2
        regexUri:
          pattern: "^/api/v1/(.*)"
          replacement: "/api/v2/$1"
        
        # 设置内部 Host
        host: "internal-api.default.svc"
        
        # 添加网关标识
        headers:
          add:
            - name: X-Gateway
              value: "edgion"
          set:
            - name: X-Api-Version
              value: "v2"
            - name: X-Original-Uri
              value: "$uri"
          remove:
            - X-Debug
```

### 测试

```bash
# 原始请求
curl -H "X-Debug: true" "https://api.example.com/api/v1/users/123?detail=true"

# 实际转发到上游的请求：
# - URI: /api/v2/users/123?detail=true
# - Host: internal-api.default.svc
# - Headers:
#   - X-Gateway: edgion (新增)
#   - X-Api-Version: v2 (设置)
#   - X-Original-Uri: /api/v1/users/123 (设置)
#   - X-Debug: (已删除)
```

---

## 与其他插件配合

### 与 BasicAuth 配合

先认证，再重写：

```yaml
spec:
  requestPlugins:
    - enable: true
      type: BasicAuth
      config:
        secretRefs:
          - name: api-users
    - enable: true
      type: ProxyRewrite
      config:
        uri: "/internal$uri"
        headers:
          set:
            - name: X-Authenticated
              value: "true"
```

### 与 CORS 配合

先处理跨域，再重写：

```yaml
spec:
  requestPlugins:
    - enable: true
      type: Cors
      config:
        allow_origins: "https://app.example.com"
        allow_methods: "GET,POST,PUT,DELETE"
    - enable: true
      type: ProxyRewrite
      config:
        host: "backend.internal.svc"
```

---

## 故障排除

### 问题 1：URI 重写不生效

**检查**：
1. 确认 `uri` 或 `regexUri` 配置正确
2. 如果使用 `regexUri`，确认正则表达式能匹配请求路径
3. 查看网关日志中的重写记录

### 问题 2：变量未替换

**检查**：
1. `$arg_xxx`：确认查询参数存在（区分大小写）
2. `$name`：确认 HTTPRoute 中定义了对应的路径参数 `/:name`
3. `$1-$9`：确认正则表达式包含对应数量的捕获组

### 问题 3：配置验证失败

**常见原因**：
1. `host` 字段与 `headers.set` 中的 Host 冲突
2. `regexUri.pattern` 正则语法错误

**解决**：查看网关启动日志中的配置验证错误信息。

---

## 性能说明

- **正则预编译**：`regexUri.pattern` 在配置加载时预编译，运行时无额外开销
- **变量解析**：按需解析，未使用的变量不会被处理
- **路径参数提取**：采用懒加载机制，只有首次访问时才从路由模式中提取
