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
    - type: ProxyRewrite
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
| `jsonBody` | Object | 否 | 结构化重写已缓冲的 JSON 请求 Body |
| `jsonBody.operations` | Array | 是（配置 `jsonBody` 时） | 按顺序执行的 RFC 6902 操作：`add`、`remove`、`replace`、`move`、`copy`、`test` |

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

### 关于 `ProxyRewriteRegex` 模式的说明

> **说明**：**不存在独立的 `ProxyRewriteRegex` 插件类型**。正则 URI 重写是单一 `ProxyRewrite` 插件（`type: ProxyRewrite`）的 `regexUri` 模式。上面场景 4 和场景 5 展示的捕获组重写，即为所谓的"ProxyRewriteRegex"行为。静态路径请使用 `uri` 字段，需要捕获入站路径片段时请使用 `regexUri` 字段（含 `pattern` + `replacement`）。

两种 URI 重写模式互斥——当 `uri` 和 `regexUri` 同时设置时，`uri` 优先，`regexUri` 被忽略：

| 模式 | 字段 | 适用场景 |
|------|------|----------|
| 静态 | `uri` | 上游路径为固定字符串（可选带 `$uri` / `$arg_*` 变量） |
| 正则 | `regexUri.pattern` + `regexUri.replacement` | 需要通过 `$1`、`$2`…… 捕获请求路径片段时 |

```yaml
# 捕获并重塑路径，同时注入标记头
config:
  regexUri:
    pattern: "^/api/v[0-9]+/users/([0-9]+)/profile$"
    replacement: "/internal/profile/$1"
  headers:
    set:
      - name: X-Rewritten-By
        value: "edgion"
```

**效果**：`/api/v2/users/42/profile` → `/internal/profile/42`（原始查询字符串自动保留）。

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

1. **JSON Body 操作**（严格按列表顺序）
2. **URI 重写**（`uri` 或 `regexUri`）
3. **Host 重写**
4. **Method 重写**
5. **Headers 修改**（add → set → remove）

---

## 注意事项

### JSON Body 重写

`jsonBody` 会启用请求 Body 缓冲，并以一个原子事务按顺序执行 RFC 6902
JSON Patch。`path` 和 `from` 使用 RFC 6901 JSON Pointer；空字符串 `""`
表示文档根。只有全部操作成功后，新 Body 才会发布，后续操作失败不会暴露前面
已经执行的局部修改。
操作配置采用严格模式：未知字段会在配置校验时被拒绝，而不是像通用 JSON Patch
文档那样被忽略。

```yaml
config:
  jsonBody:
    operations:
      - op: test
        path: /version
        value: 1
      - op: remove
        path: /credentials/password
      - op: replace
        path: /profile/tier
        value: premium
      - op: add
        path: /enabled
        value: true
      - op: copy
        from: /profile
        path: /profileSnapshot
      - op: move
        from: /legacyName
        path: /name
```

`add`、`replace` 和 `test` 必须且只能配置以下一个字段：

- `value`：JSON 字面量，显式 `null` 也受支持。
- `valueTemplate`：共享 `${type:name}` 模板。渲染结果始终作为一个 JSON
  字符串写入，不会被解析成 JSON 片段，也不会再次展开模板。模板来源缺失时请求
  返回 `400`。无法取得（为空）的 `remoteIp` 按缺失处理；已存在但内容为空的
  命名变量仍可写入空字符串。禁止引用 Secret。

数值型 `value` 遵循 Kubernetes/serde_json 解码后的数值模型。如果业务合同要求
保留原始十进制写法，或需要超出该模型的精度，请将它编码为 JSON 字符串。

```yaml
jsonBody:
  operations:
    - op: add
      path: /tenant
      valueTemplate: "${header:X-Tenant}"
```

操作数量最多 64 个，序列化后的操作计划最多 64 KiB；单个 Pointer 最多
4 KiB、64 个 token。输入和输出 Body 默认限制为 64 KiB，可通过条目级
`body.maxBodySize` 同时提高，但固定上限为 1 MiB。JSON 深度、节点数量和修改
工作量也都有硬限制；单个 JSON 数字的文本表示最多为 4 KiB。

配置 `jsonBody` 时，条目级 `body` 策略必须具备确定性：
不支持 `body.conditions`，`body.onReadFailure` 必须省略或设为
`failClose`。配置校验会拒绝 `failOpen`，因为结构化重写失败后继续转发原始
Body 会使已配置的修改变成可选行为。

只接受唯一的 `application/json` 或具体的 `application/*+json`
Content-Type；charset 如存在必须为 UTF-8，Content-Encoding 必须缺省或为
`identity`。非法 JSON、重复对象键、操作失败、`test` 失败或模板值缺失返回
`400`；不支持的媒体类型返回 `415`；输入或输出超限返回 `413`。

重写保证解析后的 JSON 语义，不保留原始空白、对象键顺序或数字原始文本表示。
数字采用标准 `serde_json` 模型，`test` 会将 `1` 和 `1.0` 等常见的解析后等价值
视为相等。模板读取 URI、Host、Method 和 Header 重写之前的请求状态。

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
  namespace: production
spec:
  requestPlugins:
    - type: ProxyRewrite
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
    - type: BasicAuth
      config:
        secretGroups:
          - name: default
            secretRefs:
              - name: api-users
    - type: ProxyRewrite
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
    - type: Cors
      config:
        allowOrigins: "https://app.example.com"
        allowMethods: "GET,POST,PUT,DELETE"
    - type: ProxyRewrite
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

---

## 相关链接

- [ResponseRewrite](./response-rewrite.md) — 响应重写
- [URL Rewrite](../gateway-api/url-rewrite.md) — Gateway API 标准 URL 重写
