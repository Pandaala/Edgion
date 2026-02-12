# ForwardAuth 插件

## 概述

ForwardAuth 插件将原始请求的关键信息（Headers、Method、URI 等）转发给外部鉴权服务，
根据鉴权服务的响应状态码决定放行或拒绝请求。

这是 API Gateway 的经典外部鉴权模式，对标：
- **Traefik**: `forwardAuth` middleware
- **nginx**: `auth_request` module
- **APISIX**: `forward-auth` plugin
- **Kong**: `forward-auth` plugin

## 功能特点

- **外部鉴权委托** - 将鉴权逻辑完全委托给外部服务，网关本身不需要了解鉴权细节
- **Header 转发** - 支持全量转发和选择性转发两种模式
- **双向 Header 传递** - 鉴权成功时可将身份信息传给上游，失败时可将错误信息返回客户端
- **优雅降级** - 鉴权服务不可用时可选择放行（degraded mode）或返回自定义错误码
- **自定义成功状态码** - 不仅支持 2xx，可自定义哪些状态码视为鉴权通过
- **连接池复用** - 基于全局共享的 HTTP Client，跨插件实例复用连接池

## 核心流程

```
Client Request
     │
     ▼
ForwardAuth Plugin
     │
     ├─── 构建鉴权请求（Header + X-Forwarded-* 元数据）
     │
     ├─── 发送到外部鉴权服务
     │
     ├─── 鉴权服务返回 2xx？
     │       │
     │       ├── 是 → 复制 upstreamHeaders 到原始请求 → 转发到上游
     │       │
     │       └── 否 → 复制 clientHeaders + 返回鉴权服务的状态码和 Body
     │
     └─── 鉴权服务不可达？
             │
             ├── allowDegradation: true → 跳过鉴权，放行
             │
             └── allowDegradation: false → 返回 statusOnError（默认 503）
```

## 配置说明

### 配置参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `uri` | string | 是 | - | 鉴权服务地址（必须以 `http://` 或 `https://` 开头） |
| `requestMethod` | string | 否 | `GET` | 发送给鉴权服务的 HTTP 方法 |
| `requestHeaders` | string[] | 否 | `null` | 转发给鉴权服务的请求头列表（见下方说明） |
| `upstreamHeaders` | string[] | 否 | `[]` | 鉴权成功时，从鉴权响应复制到原始请求的 Header |
| `clientHeaders` | string[] | 否 | `[]` | 鉴权失败时，从鉴权响应复制到客户端响应的 Header |
| `timeoutMs` | integer | 否 | `10000` | 请求超时（毫秒） |
| `successStatusCodes` | integer[] | 否 | `null` | 自定义成功状态码列表（默认任意 2xx） |
| `allowDegradation` | boolean | 否 | `false` | 鉴权服务不可用时是否放行请求 |
| `statusOnError` | integer | 否 | `503` | 鉴权服务网络错误时返回的状态码（200-599） |

### requestHeaders 行为

| 配置 | 行为 |
|------|------|
| 不设置（`null`）| 转发**全部**请求头（自动跳过 hop-by-hop 头） |
| 设为空数组 `[]` | 同上，转发全部 |
| 设为具体列表 | **仅**转发列表中指定的头 |

> **注意**：Cookie 是标准 HTTP Header（`Cookie: xxx`），在全量转发模式下自动包含，
> 在选择性模式下将 `Cookie` 加入 `requestHeaders` 列表即可转发。

### 自动添加的 X-Forwarded-* 头

无论哪种转发模式，插件都会自动为鉴权请求添加以下元数据头：

| Header | 说明 | 示例 |
|--------|------|------|
| `X-Forwarded-Host` | 原始请求的 Host | `api.example.com` |
| `X-Forwarded-Uri` | 原始请求的 URI 路径 | `/api/v1/users` |
| `X-Forwarded-Method` | 原始请求的 HTTP 方法 | `POST` |
| `X-Forwarded-Query` | 原始请求的 Query 参数 | `page=1&size=20` |

### Hop-by-Hop 头过滤

以下 HTTP hop-by-hop 头在全量转发模式下会被自动过滤（RFC 2616/7230）：

`connection`, `keep-alive`, `proxy-authenticate`, `proxy-authorization`,
`te`, `trailers`, `transfer-encoding`, `upgrade`

## 使用示例

### 示例 1：基础配置 - 转发所有 Header

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: forward-auth-basic
  namespace: default
spec:
  requestPlugins:
    - type: ForwardAuth
      config:
        uri: "http://auth-service.auth.svc:8080/verify"
        upstreamHeaders:
          - X-User-ID
          - X-User-Role
          - X-User-Email
```

转发所有原始请求头（跳过 hop-by-hop），鉴权成功后将 `X-User-ID`、`X-User-Role`、
`X-User-Email` 从鉴权响应复制到发往上游的请求中。

### 示例 2：选择性转发 Header

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: forward-auth-selective
  namespace: default
spec:
  requestPlugins:
    - type: ForwardAuth
      config:
        uri: "https://auth.example.com/api/verify"
        requestMethod: POST
        timeoutMs: 5000
        requestHeaders:
          - Authorization
          - Cookie
          - X-Request-ID
        upstreamHeaders:
          - X-User-ID
          - X-User-Role
        clientHeaders:
          - WWW-Authenticate
          - X-Auth-Error-Code
        successStatusCodes: [200, 204]
```

仅转发 `Authorization`、`Cookie`、`X-Request-ID` 三个头给鉴权服务。
使用 POST 方法、5 秒超时。仅 200 和 204 视为鉴权成功。
鉴权失败时将 `WWW-Authenticate` 和 `X-Auth-Error-Code` 返回给客户端。

### 示例 3：优雅降级模式

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: forward-auth-degraded
  namespace: default
spec:
  requestPlugins:
    - type: ForwardAuth
      config:
        uri: "http://auth-service:8080/verify"
        allowDegradation: true
        upstreamHeaders:
          - X-User-ID
```

当鉴权服务不可用（网络错误、超时等）时，跳过鉴权直接放行请求。
适用于鉴权服务作为非关键路径的场景，优先保证可用性。

### 示例 4：自定义错误状态码

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: forward-auth-custom-error
  namespace: default
spec:
  requestPlugins:
    - type: ForwardAuth
      config:
        uri: "http://auth-service:8080/verify"
        statusOnError: 403
        upstreamHeaders:
          - X-User-ID
```

当鉴权服务网络不可达时，返回 403 而不是默认的 503。

### 示例 5：配合 HTTPRoute 使用

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
  namespace: default
spec:
  parentRefs:
    - name: edgion-gateway
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
            name: forward-auth-basic
      backendRefs:
        - name: api-backend
          port: 8080
```

## 与其他网关的对比

| 特性 | Edgion | Traefik | APISIX | nginx |
|------|--------|---------|--------|-------|
| 转发全部 Header | ✅ | ✅ | ❌（仅自动头） | ✅（subrequest） |
| 选择性转发 Header | ✅ `requestHeaders` | ✅ `authRequestHeaders` | ✅ `request_headers` | ❌ |
| 上游 Header 传递 | ✅ `upstreamHeaders` | ✅ `authResponseHeaders` | ✅ `upstream_headers` | ✅ `auth_request_set` |
| 客户端 Header 传递 | ✅ `clientHeaders` | ❌ | ✅ `client_headers` | 有限（WWW-Authenticate） |
| 自定义成功状态码 | ✅ `successStatusCodes` | ❌（仅 2xx） | ❌（仅 2xx） | ❌（仅 2xx） |
| 优雅降级 | ✅ `allowDegradation` | ❌ | ✅ `allow_degradation` | ❌ |
| 自定义错误状态码 | ✅ `statusOnError` | ❌ | ✅ `status_on_error` | ❌ |
| Cookie 转发 | ✅（全量或选择性） | ✅ | ✅ | ✅ |
| TLS 支持 | ✅（rustls） | ✅ | ✅ | ✅ |
| 转发 Body | ❌ | ✅ `forwardBody` | ❌ | ❌ |
| 正则匹配 Header | ❌ | ✅ `authResponseHeadersRegex` | ❌ | ❌ |

## 鉴权服务开发指南

### 接口约定

ForwardAuth 插件对鉴权服务的接口约定如下：

**请求**：
- 方法：由 `requestMethod` 决定（默认 GET）
- 路径：由 `uri` 决定
- Header：包含原始请求头（或选择性子集）+ `X-Forwarded-*` 元数据

**响应**：
- **2xx**（或 `successStatusCodes` 中的状态码）：鉴权通过
  - 响应头中 `upstreamHeaders` 列出的头会被复制到原始请求
- **非 2xx**：鉴权拒绝
  - 状态码和 Body 原样返回给客户端
  - 响应头中 `clientHeaders` 列出的头会被复制到客户端响应

### 示例鉴权服务（Go）

```go
func authHandler(w http.ResponseWriter, r *http.Request) {
    token := r.Header.Get("Authorization")
    
    user, err := validateToken(token)
    if err != nil {
        w.Header().Set("WWW-Authenticate", "Bearer")
        w.WriteHeader(http.StatusUnauthorized)
        json.NewEncoder(w).Encode(map[string]string{
            "error": "invalid_token",
            "message": err.Error(),
        })
        return
    }
    
    // 鉴权通过：通过 Header 传递用户身份信息
    w.Header().Set("X-User-ID", user.ID)
    w.Header().Set("X-User-Role", user.Role)
    w.Header().Set("X-User-Email", user.Email)
    w.WriteHeader(http.StatusOK)
}
```

## 注意事项

1. **连接池共享**：所有 ForwardAuth 插件实例共享同一个 HTTP Client 连接池，跨实例复用 TCP 连接
2. **不跟随重定向**：HTTP Client 禁止自动跟随重定向，鉴权服务返回 301/302 会被视为鉴权失败
3. **超时保护**：默认 10 秒请求超时，建议根据鉴权服务的实际响应时间适当调整
4. **实时生效**：更新 EdgionPlugins 资源后，配置自动热重载
5. **配置验证**：URI 为空、无效 HTTP 方法、超时为 0 等会在运行时返回 500 错误
6. **Body 不转发**：当前版本不转发原始请求的 Body 到鉴权服务（大多数鉴权场景不需要）

## 常见问题

### Q: 全量转发和选择性转发该选哪个？

A:
- **全量转发**（不设 `requestHeaders`）：简单，鉴权服务可以根据需要使用任何原始头。
  适合内部鉴权服务、对安全要求不高的场景。
- **选择性转发**（设置 `requestHeaders`）：只传递必要的头，减少数据传输，更安全。
  适合外部鉴权服务、需要最小权限原则的场景。

### Q: Cookie 怎么转发？

A: Cookie 是标准的 HTTP Header。全量转发模式下自动包含；选择性模式下把 `Cookie`
加入 `requestHeaders` 列表即可。

### Q: 鉴权服务返回的 Body 会传给客户端吗？

A: 鉴权**失败**时，鉴权服务的响应 Body 会原样返回给客户端（适合传递错误详情）。
鉴权**成功**时，Body 被忽略。

### Q: 多个 ForwardAuth 插件可以串联吗？

A: 可以。多个 ForwardAuth 插件按 `requestPlugins` 数组中的顺序依次执行。
任何一个返回非 2xx 即拒绝请求。

### Q: `allowDegradation` 有安全风险吗？

A: 有。开启后，鉴权服务不可用时请求会直接放行（无鉴权），
只适合鉴权不是关键安全屏障的场景（如 A/B 测试、非敏感 API）。
对于安全关键的 API，建议保持默认的 `false`。
