# EdgionDSL 实战示例集

本文档收录了 EdgionDSL 在真实生产场景中的完整配置示例，并在最后汇总了最佳实践与常见反模式。每个示例均包含完整的 `EdgionPlugins` CRD 配置，可直接参考使用。

---

## 1. 认证与鉴权

### 1.1 API Key 验证

最简单的认证场景：从请求头中取出 API Key，与预期值比对，通过后在上下文中标记认证状态以便后续插件使用。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: api-key-check
  namespace: production
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "api-key-check"
        errorPolicy: deny
        source: |
          let key = req.header("X-API-Key")
          if key == nil {
            return deny(401, "Missing API key")
          }
          if key != "expected-secret-key" {
            return deny(403, "Invalid API key")
          }
          ctx.set("authenticated", "true")
          return next()
```

说明：

- `deny(401, ...)` 用于"凭据缺失"，`deny(403, ...)` 用于"凭据无效"，语义上更准确。
- `ctx.set("authenticated", "true")` 将认证结果写入请求上下文，供后续插件通过 `ctx.get("authenticated")` 读取。若需记录日志，必须显式声明 `from: ctx` 的 `AccessLogExtern` 字段，见下方审计示例。
- `errorPolicy: deny` 表示脚本本身出错时也拒绝请求，避免因脚本异常导致认证绕过。

---

### 1.2 JWT Token 提取与路由标记

从 `Authorization` 请求头提取 Bearer Token，对 Base64 编码的 Payload 段进行解码，再用 `json_get` 提取声明（claims），并将关键字段写入上下文供下游使用。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: jwt-claim-extractor
  namespace: production
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "jwt-claim-extractor"
        errorPolicy: deny
        source: |
          let auth = req.header("Authorization")
          if auth == nil {
            return deny(401, "Authorization header required")
          }
          if !auth.starts_with("Bearer ") {
            return deny(401, "Authorization must be Bearer token")
          }

          // 取出 token 字符串（去掉 "Bearer " 前缀，长度为 7）
          let token = substr(auth, 7, len(auth))

          // JWT 由三段 base64url 组成，取第二段（Payload）
          let parts = split(token, ".")
          let mut payload_b64 = nil
          let mut idx = 0
          for part in parts {
            if idx == 1 {
              payload_b64 = part
              break
            }
            idx = idx + 1
          }

          if payload_b64 == nil {
            return deny(400, "Malformed JWT: missing payload segment")
          }

          // 解码 Payload
          let payload_json = base64_decode(payload_b64)
          if payload_json == nil {
            return deny(400, "Malformed JWT: payload base64 decode failed")
          }

          // 提取常用声明
          let sub = json_get(payload_json, "$.sub")
          let role = json_get(payload_json, "$.role")
          let tenant = json_get(payload_json, "$.tenant_id")

          if sub == nil {
            return deny(401, "JWT missing sub claim")
          }

          // 将声明写入上下文和请求头，传递给上游
          ctx.set("user_id", sub)
          req.set_header("X-User-Id", sub)

          if role != nil {
            ctx.set("user_role", role)
            req.set_header("X-User-Role", role)
          }

          if tenant != nil {
            ctx.set("tenant_id", tenant)
            req.set_header("X-Tenant-Id", tenant)
          }

          return next()
```

说明：

- 本示例仅做结构解析，**不做签名验证**。在生产环境中，应将签名校验交给专用的 JWT Auth 插件，DSL 负责在验证通过后进一步提取声明。
- `json_get` 返回选中的字符串、整数、布尔值、序列化后的数组/对象或 `nil`；使用必需声明前应检查 `nil`。

---

### 1.3 gRPC 认证检查

通过 `req.protocol` 检测当前请求是否为 gRPC，再校验 `authorization` 头，未通过则返回标准 gRPC 状态码而非 HTTP 错误。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: grpc-auth-check
  namespace: production
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "grpc-auth-check"
        errorPolicy: deny
        source: |
          // 非 gRPC 请求直接放行（由其他插件处理）
          if req.protocol != "grpc" {
            return next()
          }

          let token = req.header("authorization")
          if token == nil {
            // gRPC 状态码 16 = UNAUTHENTICATED
            return grpc_error(16, "missing authorization token")
          }

          if !token.starts_with("Bearer ") {
            return grpc_error(16, "authorization must be Bearer token")
          }

          // 检查服务级别权限
          let service = req.grpc_service
          if service == "internal.AdminService" {
            let admin_key = req.header("x-admin-key")
            if admin_key == nil {
              // gRPC 状态码 7 = PERMISSION_DENIED
              return grpc_error(7, "AdminService requires x-admin-key header")
            }
          }

          return next()
```

说明：

- gRPC 状态码通过 HTTP/2 trailers 传递，tonic 等客户端可正确解析。
- 常用状态码：`7` PERMISSION_DENIED、`16` UNAUTHENTICATED。
- `grpc_error()` 支持请求和响应两个阶段。请求阶段直接发送 trailers-only 响应；响应阶段中止上游响应，将准确的状态码和可选消息保留给统一终止响应写入器。线缆 HTTP 状态为 `200`，`grpc_status` 独立记录 gRPC 应用状态。

---

## 2. 请求路由与改写

### 2.1 基于 User-Agent 的客户端分类

解析 `User-Agent` 请求头，识别移动端与桌面端客户端，并注入标准化的 `X-Client-Type` 头，供上游服务按设备类型差异化处理。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ua-client-classify
  namespace: production
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "ua-client-classify"
        source: |
          let ua = req.header("User-Agent")

          if ua == nil {
            req.set_header("X-Client-Type", "unknown")
            return next()
          }

          // 移动端特征关键词检测（顺序靠前的优先）
          if ua.contains("Mobile") || ua.contains("Android") || ua.contains("iPhone") {
            req.set_header("X-Client-Type", "mobile")
            ctx.set("client_type", "mobile")
            return next()
          }

          // 平板端
          if ua.contains("iPad") || ua.contains("Tablet") {
            req.set_header("X-Client-Type", "tablet")
            ctx.set("client_type", "tablet")
            return next()
          }

          // 爬虫/机器人
          if ua.contains("Bot") || ua.contains("Crawler") || ua.contains("Spider") {
            req.set_header("X-Client-Type", "bot")
            ctx.set("client_type", "bot")
            return next()
          }

          // 默认为桌面端
          req.set_header("X-Client-Type", "desktop")
          ctx.set("client_type", "desktop")
          return next()
```

---

### 2.2 基于路径的上游选择

通过检查请求路径前缀，写入上下文变量，配合动态上游插件实现路径级别的服务路由。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: path-based-upstream
  namespace: production
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "path-based-upstream"
        source: |
          let path = req.path()

          // /api/v1/* 路由到旧版服务
          if path.starts_with("/api/v1/") {
            ctx.set("upstream_cluster", "backend-v1")
            ctx.set("api_version", "v1")
            return next()
          }

          // /api/v2/* 路由到新版服务
          if path.starts_with("/api/v2/") {
            ctx.set("upstream_cluster", "backend-v2")
            ctx.set("api_version", "v2")
            return next()
          }

          // /internal/* 仅允许内网访问
          if path.starts_with("/internal/") {
            // 该网络来源规则故意要求物理传输对端。
            let ip = req.direct_peer_ip()
            if !ip_in_cidr(ip, "10.0.0.0/8") {
              return deny(403, "internal endpoints are not publicly accessible")
            }
            ctx.set("upstream_cluster", "backend-internal")
            return next()
          }

          // 其他路径路由到默认服务
          ctx.set("upstream_cluster", "backend-default")
          return next()
```

---

### 2.3 A/B 测试流量标记

使用 `hmac_sha256` 对客户端 IP 进行确定性哈希，将流量按比例分配到不同实验组，同一客户端始终落入相同分组。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ab-test-marker
  namespace: production
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "ab-test-marker"
        source: |
          // 优先使用已有的实验分组 Cookie，避免重复计算
          let existing = req.cookie("ab_group")
          if existing != nil {
            ctx.set("ab_group", existing)
            req.set_header("X-AB-Group", existing)
            return next()
          }

          let ip = req.remote_ip()

          // 使用固定 salt 对 IP 做 HMAC，得到确定性哈希
          let hash = hmac_sha256("ab-test-salt-2024", ip)

          // 取哈希最后两字符转为数值，模 100 得到 0-99 的桶编号
          let hash_len = len(hash)
          let tail = substr(hash, hash_len - 2, hash_len)
          let bucket = hex_to_int(tail)

          // 20% 流量进入实验组 B，其余进入控制组 A
          let mut group = "A"
          if bucket < 51 {
            group = "B"
          }

          ctx.set("ab_group", group)
          req.set_header("X-AB-Group", group)
          return next()
```

说明：

- `hmac_sha256` 的输出是十六进制字符串，对同一输入始终返回相同结果，保证分流的幂等性。
- 通过 Cookie 短路可以避免每次请求重新计算，也让用户在跨 IP 场景下保持一致体验。

---

## 3. 安全防护

### 3.1 内网 IP 限流豁免

检查客户端 IP 是否属于内网 CIDR 段，若是则在上下文中写入豁免标志，供下游限流插件读取后跳过限速逻辑。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: internal-ip-bypass
  namespace: production
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "internal-ip-bypass"
        source: |
          // 该内网来源规则故意使用物理传输对端。
          let ip = req.direct_peer_ip()

          // 检查常见内网段
          if ip_in_cidr(ip, "10.0.0.0/8") {
            ctx.set("rate_limit_bypass", "true")
            ctx.set("network_zone", "internal-10")
            return next()
          }

          if ip_in_cidr(ip, "172.16.0.0/12") {
            ctx.set("rate_limit_bypass", "true")
            ctx.set("network_zone", "internal-172")
            return next()
          }

          if ip_in_cidr(ip, "192.168.0.0/16") {
            ctx.set("rate_limit_bypass", "true")
            ctx.set("network_zone", "internal-192")
            return next()
          }

          // 外部请求，不设豁免标志
          ctx.set("rate_limit_bypass", "false")
          ctx.set("network_zone", "external")
          return next()
```

说明：

- `ip_in_cidr` 同时支持 IPv4 和 IPv6 地址，无需额外处理地址类型。
- 下游限流插件通过 `ctx.get("rate_limit_bypass")` 读取标志，决定是否跳过限速。

---

### 3.2 请求体大小限制

使用已捕获请求体的实际字节长度，在转发前拒绝超过阈值的大请求。该方式也覆盖无法从请求头预知大小的 chunked 请求。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: body-size-limit
  namespace: production
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "body-size-limit"
        source: |
          // 限制请求体不超过 10 MB（10 * 1024 * 1024 = 10485760）
          let max_bytes = 10485760
          if req.body.byte_len() > max_bytes {
            return deny(413, "Request body is too large")
          }

          return next()
```

说明：

- `req.body.byte_len()` 返回已捕获请求体的原始字节数，不会把请求体转换成字符串。
- 无论共享请求体缓存留在内存还是溢写到磁盘，同一脚本都可正常工作。
- 网关的请求体物理上限仍是传输层权威限制；DSL 阈值只能与其相等或更低。

---

### 3.3 敏感 Header 清理

遍历所有请求头名称，移除以 `x-internal-` 开头的头，防止外部客户端通过注入内部路由头绕过访问控制。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: strip-internal-headers
  namespace: production
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "strip-internal-headers"
        source: |
          // 收集需要删除的头名称
          // 注意：不能在 for 循环中直接修改正在迭代的列表
          // 先收集，再统一删除
          let mut to_remove = ""

          for name in req.header_names() {
            if name.starts_with("x-internal-") {
              // 用逗号分隔拼接需要删除的头名称列表
              if len(to_remove) == 0 {
                to_remove = name
              } else {
                to_remove = to_remove + "," + name
              }
            }
          }

          // 二次遍历删除
          if len(to_remove) > 0 {
            let names = split(to_remove, ",")
            for name in names {
              req.remove_header(name)
            }
          }

          return next()
```

说明：

- 不能在 `for ... in req.header_names()` 循环体内同时调用 `req.remove_header()`，因为这会修改正在迭代的底层集合。应先收集再删除。
- 如果确定内部头数量有限（如已知只有三四个固定名称），直接逐个调用 `req.remove_header()` 更简洁，无需循环。

---

## 4. 响应处理

### 4.1 安全响应头注入

在 `upstreamResponsePlugins` 阶段，为每个响应统一注入标准安全响应头，减少上游服务重复配置的负担。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: security-headers
  namespace: production
spec:
  upstreamResponsePlugins:
    - type: Dsl
      config:
        name: "inject-security-headers"
        source: |
          // 防止点击劫持
          resp.set_header("X-Frame-Options", "DENY")

          // 防止 MIME 嗅探
          resp.set_header("X-Content-Type-Options", "nosniff")

          // XSS 保护（现代浏览器已内置，但保留兼容旧浏览器）
          resp.set_header("X-XSS-Protection", "1; mode=block")

          // 强制 HTTPS（1 年）
          resp.set_header("Strict-Transport-Security", "max-age=31536000; includeSubDomains")

          // 限制 Referrer 信息泄露
          resp.set_header("Referrer-Policy", "strict-origin-when-cross-origin")

          // 基础 CSP：仅允许同源资源（根据实际需要放宽）
          resp.set_header("Content-Security-Policy", "default-src 'self'")

          return next()
```

说明：

- 响应阶段使用 `resp.set_header()` 写入或覆盖响应头，`resp.append_header()` 用于追加多值头。
- 如需根据路径或上游响应状态码条件注入，可先读 `resp.status` 或 `req.path()` 做判断。

---

### 4.2 缓存控制

根据请求路径前缀，为静态资源和 API 响应分别设置合适的 `Cache-Control` 策略。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: cache-control-policy
  namespace: production
spec:
  upstreamResponsePlugins:
    - type: Dsl
      config:
        name: "cache-control-policy"
        source: |
          let path = req.path()
          let status = resp.status

          // 错误响应不缓存
          if status >= 400 {
            resp.set_header("Cache-Control", "no-store")
            return next()
          }

          // 静态资源：长期缓存，不可变（fingerprint 文件名保证更新）
          if path.starts_with("/static/") || path.starts_with("/assets/") {
            resp.set_header("Cache-Control", "public, max-age=31536000, immutable")
            return next()
          }

          // 图片和字体
          if path.ends_with(".png") || path.ends_with(".jpg") || path.ends_with(".woff2") {
            resp.set_header("Cache-Control", "public, max-age=86400")
            return next()
          }

          // API 接口：禁止缓存
          if path.starts_with("/api/") {
            resp.set_header("Cache-Control", "no-cache, no-store, must-revalidate")
            resp.set_header("Pragma", "no-cache")
            return next()
          }

          // 默认：协商缓存
          resp.set_header("Cache-Control", "no-cache")
          return next()
```

---

### 4.3 调试头清理

在将响应发送给客户端之前，移除上游服务返回的内部调试头，避免泄露服务内部信息。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: strip-debug-headers
  namespace: production
spec:
  upstreamResponsePlugins:
    - type: Dsl
      config:
        name: "strip-debug-headers"
        source: |
          // 移除常见的内部调试/追踪响应头
          resp.remove_header("X-Debug-Info")
          resp.remove_header("X-Internal-Trace-Id")
          resp.remove_header("X-Server-Name")
          resp.remove_header("X-Pod-Name")
          resp.remove_header("X-Node-Name")
          resp.remove_header("X-Upstream-Latency")
          resp.remove_header("Server")

          // 遍历并清理 x-debug-* 前缀的头
          let mut to_remove = ""
          for name in resp.header_names() {
            if name.starts_with("x-debug-") {
              if len(to_remove) == 0 {
                to_remove = name
              } else {
                to_remove = to_remove + "," + name
              }
            }
          }

          if len(to_remove) > 0 {
            let names = split(to_remove, ",")
            for name in names {
              resp.remove_header(name)
            }
          }

          return next()
```

---

## 5. 可观测性

### 5.1 请求审计日志

使用 `ctx.set()` 存储请求元数据，再通过 `EdgionPlugins.spec.accessLogExtern` 显式选择访问日志字段。该声明在终态记录日志时读取，字段输出到 `ext`，不作为请求插件执行。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: request-audit-log
  namespace: production
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "request-audit-log"
        source: |
          let method = req.method()
          let path = req.path()
          let ip = req.remote_ip()
          // 为显式声明的访问日志字段存储值
          ctx.set("audit_method", method)
          ctx.set("audit_path", path)
          ctx.set("audit_ip", ip)

          // 记录认证信息（脱敏：只记录是否存在）
          let auth = req.header("Authorization")
          if auth != nil {
            ctx.set("has_auth", "true")
          } else {
            ctx.set("has_auth", "false")
          }

          // 记录 User-Agent 前 64 字符
          let ua = req.header("User-Agent")
          if ua != nil {
            let ua_short = substr(ua, 0, 64)
            ctx.set("user_agent_short", ua_short)
          }

          return next()
  accessLogExtern:
    - key: audit_method
      from: ctx
      name: audit_method
    - key: audit_path
      from: ctx
      name: audit_path
    - key: audit_ip
      from: ctx
      name: audit_ip
    - key: has_auth
      from: ctx
      name: has_auth
    - key: user_agent_short
      from: ctx
      name: user_agent_short
```

说明：

- 按常规方式将此 EdgionPlugins 资源挂载到路由。只有显式声明且存在的上下文键才会投影到 `ext`；其他 `ctx.set()` 值仅保留在请求上下文中。
- 所有 `ctx` 键默认脱敏（默认 `replace` 模式输出 `***`）。如需可读字段，必须在 `EdgionGatewayConfig.spec.accessLogExtern.unmaskedKeys.ctx` 中显式加入经过审查的键；本例不关闭脱敏。
- `jwt_claims`、`oidc_claims` 和 `jwe_payload` 无论是否配置免脱敏键或选择其他敏感输出模式，都禁止投影。不要将凭据或完整 claims 复制到另一个键，也不要通过 `log()` 绕过该边界。
- 挂载、脱敏及禁止字段的详细规则见 [AccessLogExtern](../../../../../ops-guide/observability/access-log.md)。本例仅记录认证信息是否存在，不记录凭据值。

---

### 5.2 请求耗时标记

在请求阶段记录时间戳，在响应阶段计算耗时并写入响应头，方便客户端和监控系统做耗时分析。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: latency-marker
  namespace: production
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "latency-marker-req"
        source: |
          // 记录请求进入时间（Unix 毫秒）
          let start_ms = time_now()
          ctx.set("req_start_ms", to_str(start_ms))

          // 生成请求 ID 用于关联日志
          let req_id = uuid()
          ctx.set("request_id", req_id)
          req.set_header("X-Request-Id", req_id)

          return next()

  upstreamResponsePlugins:
    - type: Dsl
      config:
        name: "latency-marker-resp"
        source: |
          let end_ms = time_now()
          let start_str = ctx.get("req_start_ms")

          if start_str != nil {
            let start_ms = to_int(start_str)
            if start_ms != nil {
              let elapsed_ms = end_ms - start_ms
              resp.set_header("X-Upstream-Latency-Ms", to_str(elapsed_ms))
              ctx.set("upstream_latency_ms", to_str(elapsed_ms))
            }
          }

          // 将请求 ID 透传到响应，便于客户端关联
          let req_id = ctx.get("request_id")
          if req_id != nil {
            resp.set_header("X-Request-Id", req_id)
          }

          return next()
  accessLogExtern:
    - key: upstream_latency_ms
      from: ctx
      name: upstream_latency_ms
```

说明：

- `time_now()` 返回 Unix 毫秒时间戳（`Int` 类型）。
- 上下文变量只能存储字符串，因此需要 `to_str()` 和 `to_int()` 来回转换数值。
- `ctx.set("upstream_latency_ms", ...)` 将值存入请求上下文。脚本下方显式的 `from: ctx` 声明将其选入 `ext`，但默认仍会脱敏。若需数值聚合，须按 [AccessLogExtern](../../../../../ops-guide/observability/access-log.md) 配置经过审查的免脱敏键。

---

## 6. 最佳实践

### 6.1 尽早返回

认证检查、必要字段校验等前置条件应在脚本开头完成，失败时立即 `return deny()`，避免执行不必要的后续逻辑。

```yaml
# 推荐：先检查必要条件，不满足立即返回
source: |
  let key = req.header("X-API-Key")
  if key == nil {
    return deny(401, "missing api key")
  }
  // 后续逻辑确保 key 不为 nil
  if key != "valid-key" {
    return deny(403, "invalid api key")
  }
  ctx.set("authenticated", "true")
  return next()
```

### 6.2 善用上下文变量

使用上下文变量在同一请求内共享值；这些值不会自动记录日志。仅通过 `AccessLogExtern from: ctx` 选择所需字段，并保留脱敏与绝对禁止字段规则。建议使用小写下划线等统一命名方式。

```yaml
source: |
  ctx.set("user_id", sub)          // 仅存入请求上下文；记录日志需要显式声明
  ctx.set("tenant_id", tenant)     // 仅存入请求上下文；记录日志需要显式声明
  ctx.set("ab_group", group)       // 仅存入请求上下文；记录日志需要显式声明
  return next()
```

### 6.3 错误策略选择

| 场景 | 推荐策略 | 原因 |
|------|----------|------|
| 通用流量处理（Header 注入、日志标记） | `Ignore` | 脚本出错不应影响正常请求 |
| 认证/鉴权检查 | `Deny` | 脚本出错应安全失败（fail-closed） |
| 合规审计、必须记录 | `DenyWith` | 返回自定义错误响应，给客户端明确提示 |

```yaml
# 安全敏感场景：脚本出错应拒绝请求
config:
  name: "auth-check"
  errorPolicy: deny
  source: |
    ...

# 增强场景：脚本出错应放行请求
config:
  name: "header-enrichment"
  errorPolicy: Ignore
  source: |
    ...
```

### 6.4 资源限制调整原则

默认限制适合绝大多数场景，不要盲目调大。在以下情况下才考虑调整：

- **降低** `maxLoopIterations`（如 `20`）：已知遍历的列表长度有限，调小可更早发现异常。
- **提高** `maxStringLen`（如 `32768`）：需要从 `read()`、`json_get()` 返回较长字符串，或处理较长 Base64 内容。流式请求体谓词不需要分配完整字符串。
- 不应将 `maxSteps` 调大来"修复"无限循环，应检查脚本逻辑。

### 6.5 字节码模式

高频路由上的 DSL 插件建议使用预编译字节码，跳过每次配置加载时的编译开销。

```yaml
config:
  name: "hot-path-check"
  # 使用预编译字节码替代 source
  bytecode: "BASE64_ENCODED_BYTECODE_HERE"
```

编译字节码的方法见 [字节码指南](./bytecode.md)。

### 6.6 避免复杂逻辑

DSL 适合轻量级、决策导向的处理逻辑（认证前置、Header 注入、路由标记）。复杂业务逻辑（数据库查询、远程调用、复杂数据转换）应开发专用原生 Rust 插件：

- 原生插件可以访问异步运行时，发起真正的网络请求。
- 原生插件没有步数和循环次数的沙箱限制。
- DSL 脚本的调试成本高于类型安全的 Rust 代码。

---

## 7. 常见反模式

### 反模式一：在循环中调用大量函数

```yaml
# 错误示范：每次循环都调用多个函数，容易触发 maxCallCount 限制
source: |
  for name in req.header_names() {
    let val = req.header(name)           # 函数调用 +1
    let lower = to_lower(val)            # 函数调用 +1
    let encoded = base64_encode(lower)   # 函数调用 +1
    log("header: " + name + "=" + encoded)  # 函数调用 +1
  }
  return next()
```

```yaml
# 正确做法：仅在必要时调用函数，减少单次循环的调用数量
source: |
  for name in req.header_names() {
    if name.starts_with("x-log-") {
      let val = req.header(name)
      log("header: " + name + "=" + val)
    }
  }
  return next()
```

### 反模式二：大量字符串拼接

```yaml
# 错误示范：拼接所有请求头，字符串可能超过 maxStringLen 限制
source: |
  let mut all_headers = ""
  for name in req.header_names() {
    let val = req.header(name)
    all_headers = all_headers + name + ": " + val + "\n"
  }
  log(all_headers)  # 可能触发 maxStringLen 错误
  return next()
```

```yaml
# 正确做法：只记录关键头，逐条 log 而非拼接
source: |
  let auth = req.header("Authorization")
  let ct = req.header("Content-Type")
  if auth != nil {
    log("auth header present")
  }
  if ct != nil {
    log("content-type: " + ct)
  }
  return next()
```

### 反模式三：未检查 nil 值

```yaml
# 错误示范：直接对可能为 nil 的值调用方法
source: |
  let ua = req.header("User-Agent")
  # ua 可能为 nil，nil.starts_with() 返回 false 而不是崩溃
  # 但下面的字符串拼接会把 nil 转成字面量 "nil"
  log("ua: " + ua)  # 当 ua 为 nil 时，输出 "ua: nil"，不是期望行为
  return next()
```

```yaml
# 正确做法：先检查 nil，再使用
source: |
  let ua = req.header("User-Agent")
  if ua != nil {
    log("ua: " + ua)
    if ua.starts_with("Mozilla") {
      req.set_header("X-Browser-Client", "true")
    }
  }
  return next()
```

## 响应阶段 gRPC 错误

使用准确的 `RESOURCE_EXHAUSTED` 状态码（`8`）和消息中止失败的上游 gRPC 响应。客户端收到 HTTP `200` 和 gRPC 状态 `8`；访问日志记录 `grpc_status: 8`。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: grpc-response-error
  namespace: production
spec:
  upstreamResponsePlugins:
    - type: Dsl
      config:
        name: grpc-response-error
        source: |
          if req.protocol == "grpc" && resp.status >= 500 {
            return grpc_error(8, "upstream capacity exhausted")
          }
          return next()
```

---

## 相关文档

- [DSL 插件概述与配置参数](./README.md)
- [EdgionDSL 语言语法参考](./language-syntax.md)
- [API 参考手册](./api-reference.md)
- [沙盒安全模型](./sandbox-security.md)
- [字节码指南](./bytecode.md)
