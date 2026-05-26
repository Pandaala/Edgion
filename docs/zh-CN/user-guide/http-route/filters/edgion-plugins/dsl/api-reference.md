# EdgionDSL API 参考手册

本文档列出 EdgionDSL 所有内置函数，按功能分类组织。每个函数均给出完整签名、参数说明、返回值、限制条件和使用示例。

**类型说明**：EdgionDSL 有 6 种运行时类型：`Str`（字符串）、`Int`（64 位整数）、`Bool`（布尔值）、`Nil`（空值/缺失）、`List`（字符串列表，不可由用户直接构造）、`Map`（有序字符串键映射表；由 `http.call()` 返回，也可通过 `{key: expr}` 字面量构造）。

**阶段说明**：部分函数仅在特定执行阶段有效。`req.*` 写操作和 `grpc_error()` 仅在请求阶段有意义；`resp.*` 读写操作仅在响应阶段有效。

---

## 目录

- [请求读取（req.*）](#请求读取-req)
- [请求元数据（req.*）](#请求元数据-req)
- [请求修改（req.*）](#请求修改-req)
- [响应读取（resp.*）](#响应读取-resp)
- [响应修改（resp.*）](#响应修改-resp)
- [上下文存储（ctx.*）](#上下文存储-ctx)
- [字符串处理](#字符串处理)
- [编码与解码](#编码与解码)
- [哈希与加密](#哈希与加密)
- [正则表达式](#正则表达式)
- [时间](#时间)
- [JSON](#json)
- [网络](#网络)
- [Redis（redis.*）](#redisredis)
- [控制流函数](#控制流函数)
- [其他工具](#其他工具)

---

## 请求读取（req.*）

### req.header

**签名**：`req.header(name)` → `Str | Nil`

读取请求头中指定名称的值。Header 名称不区分大小写。若请求中不存在该 Header，返回 `nil`。若存在多个同名 Header，返回第一个值。

**参数**：
- `name`（Str）— Header 名称，不区分大小写

**返回值**：Header 的字符串值，或 `nil`（Header 不存在）

**示例**：
```
let token = req.header("Authorization")
if token == nil {
  return deny(401, "missing Authorization")
}
let bearer = substr(token, 7, len(token))
```

---

### req.method

**签名**：`req.method` → `Str`

返回当前请求的 HTTP 方法，如 `"GET"`、`"POST"`、`"PUT"`、`"DELETE"` 等，始终为大写。

**返回值**：HTTP 方法字符串

**示例**：
```
if req.method != "GET" and req.method != "POST" {
  return deny(405, "method not allowed")
}
```

---

### req.path

**签名**：`req.path` → `Str`

返回请求的路径部分（不含查询字符串），始终以 `/` 开头。例如请求 `/api/v1/users?page=1` 时返回 `"/api/v1/users"`。

**返回值**：请求路径字符串

**示例**：
```
let path = req.path
if path == "/health" {
  return deny(200, "ok")
}
```

---

### req.query

**签名**：`req.query(name)` → `Str | Nil`

读取 URL 查询参数中指定名称的值。若该参数不存在，返回 `nil`。若同名参数出现多次，返回第一个值。

**参数**：
- `name`（Str）— 查询参数名称，区分大小写

**返回值**：参数值字符串，或 `nil`

**示例**：
```
let page = req.query("page")
if page == nil {
  page = "1"
}
```

---

### req.query_string

**签名**：`req.query_string` → `Str | Nil`

返回完整的查询字符串，不含前导 `?`。若 URL 中没有查询字符串，返回 `nil`。

**返回值**：完整查询字符串，或 `nil`

**示例**：
```
let qs = req.query_string
if qs != nil {
  req.set_header("X-Query-String", qs)
}
```

---

### req.cookie

**签名**：`req.cookie(name)` → `Str | Nil`

从请求的 `Cookie` Header 中读取指定名称的 Cookie 值。若该 Cookie 不存在，返回 `nil`。

**参数**：
- `name`（Str）— Cookie 名称，区分大小写

**返回值**：Cookie 值字符串，或 `nil`

**示例**：
```
let session = req.cookie("session_id")
if session == nil {
  return deny(401, "session required")
}
req.set_header("X-Session-Id", session)
```

---

### req.client_ip

**签名**：`req.client_ip` → `Str`

返回经过 `real-ip` 插件处理后的客户端 IP 地址。若未配置 `real-ip` 插件，则与 `req.remote_ip` 相同，返回直接连接的远端地址。

**返回值**：IP 地址字符串（IPv4 或 IPv6 格式）

**示例**：
```
let ip = req.client_ip
if ip_in_cidr(ip, "10.0.0.0/8") {
  req.set_header("X-Internal-Request", "true")
}
```

---

### req.remote_ip

**签名**：`req.remote_ip` → `Str`

返回 TCP 连接的直接对端 IP 地址，不经过任何代理头处理。这是底层连接的实际远端地址。

**返回值**：IP 地址字符串（IPv4 或 IPv6 格式）

**示例**：
```
let remote = req.remote_ip
log("direct connection from: " + remote)
```

---

### req.path_param

**签名**：`req.path_param(name)` → `Str | Nil`

读取路由路径模板中的命名参数值。例如路由为 `/users/{userId}`，请求路径为 `/users/42` 时，`req.path_param("userId")` 返回 `"42"`。若参数不存在，返回 `nil`。

**参数**：
- `name`（Str）— 路径参数名称

**返回值**：路径参数值字符串，或 `nil`

**示例**：
```
let user_id = req.path_param("userId")
if user_id == nil {
  return deny(400, "missing path parameter: userId")
}
req.set_header("X-User-Id", user_id)
```

---

### req.header_names

**签名**：`req.header_names` → `List`

返回当前请求中所有 Header 名称的列表。最多返回 256 个名称，超出部分被截断。列表元素均为字符串，顺序与请求中出现的顺序一致。

**返回值**：包含所有请求 Header 名称的 `List`

**限制**：最多返回 256 个 Header 名称

**示例**：
```
let names = req.header_names
for name in names {
  log("header: " + name)
}
```

---

### req.scheme

**签名**：`req.scheme` → `Str`

返回请求使用的协议方案。优先读取 `X-Forwarded-Proto` Header，其次读取 `X-Scheme` Header，若均不存在则返回 `"http"`。

**返回值**：协议方案字符串，通常为 `"http"` 或 `"https"`

**示例**：
```
if req.scheme != "https" {
  return deny(400, "HTTPS required")
}
```

---

### req.host

**签名**：`req.host` → `Str | Nil`

返回请求中的 `Host` Header 值。若请求未携带 `Host` Header，返回 `nil`。

**返回值**：Host 字符串（可能含端口，如 `"example.com:8080"`），或 `nil`

**示例**：
```
let host = req.host
if host == nil or host != "api.example.com" {
  return deny(403, "invalid host")
}
```

---

### req.uri

**签名**：`req.uri` → `Str`

返回完整的请求 URI，包含路径和查询字符串。若存在查询字符串，格式为 `path?query`；否则仅返回路径。

**返回值**：完整 URI 字符串

**示例**：
```
let uri = req.uri
ctx.set("original_uri", uri)
req.set_uri("/api/v2" + req.path)
```

---

### req.content_type

**签名**：`req.content_type` → `Str | Nil`

读取请求的 `Content-Type` Header 值。若不存在，返回 `nil`。等价于 `req.header("Content-Type")`，但语义更明确。

**返回值**：Content-Type 字符串，或 `nil`

**示例**：
```
let ct = req.content_type
if ct != nil and regex_find(ct, "application/json") == nil {
  return deny(415, "only application/json is accepted")
}
```

---

### req.has_header

**签名**：`req.has_header(name)` → `Bool`

检查请求是否包含指定名称的 Header，不返回值，仅判断存在性。Header 名称不区分大小写。

**参数**：
- `name`（Str）— Header 名称

**返回值**：`true` 表示 Header 存在，`false` 表示不存在

**示例**：
```
if not req.has_header("X-Request-Id") {
  req.set_header("X-Request-Id", uuid())
}
```

---

## 请求元数据（req.*）

### req.protocol

**签名**：`req.protocol` → `Str`

返回当前请求的协议类型标识。可能的值为：
- `"http"` — 普通 HTTP/1.1 或 HTTP/2 请求
- `"grpc"` — gRPC 请求（`Content-Type: application/grpc`）
- `"grpc-web"` — gRPC-Web 请求
- `"websocket"` — WebSocket 升级请求

**返回值**：协议类型字符串

**示例**：
```
let proto = req.protocol
if proto == "grpc" {
  let svc = req.grpc_service
  req.set_header("X-Grpc-Service", svc)
}
```

---

### req.grpc_service

**签名**：`req.grpc_service` → `Str | Nil`

从 gRPC 请求路径中解析服务名称。gRPC 路径格式为 `/{package}.{ServiceName}/{MethodName}`，本函数返回包含包名的完整服务名，例如 `"example.helloworld.Greeter"`。若请求不是 gRPC 协议或路径格式不符，返回 `nil`。

**返回值**：gRPC 服务名字符串，或 `nil`

**示例**：
```
let svc = req.grpc_service
if svc != nil and svc != "example.auth.AuthService" {
  return grpc_error(12, "service not found")
}
```

---

### req.grpc_method

**签名**：`req.grpc_method` → `Str | Nil`

从 gRPC 请求路径中解析方法名称。例如路径为 `/helloworld.Greeter/SayHello` 时，返回 `"SayHello"`。若请求不是 gRPC 协议或路径格式不符，返回 `nil`。

**返回值**：gRPC 方法名字符串，或 `nil`

**示例**：
```
let method = req.grpc_method
if method == "AdminRpc" {
  let token = req.header("Authorization")
  if token == nil {
    return grpc_error(16, "unauthenticated")
  }
}
```

---

### req.content_length

**签名**：`req.content_length` → `Int | Nil`

读取请求的 `Content-Length` Header 并解析为整数。若 Header 不存在，或值无法解析为整数，返回 `nil`。

**返回值**：内容长度（字节数），或 `nil`

**示例**：
```
let cl = req.content_length
if cl != nil and cl > 1048576 {
  return deny(413, "request body too large")
}
```

---

### req.client_cert_cn

**签名**：`req.client_cert_cn` → `Str | Nil`

读取 mTLS 客户端证书的 Common Name（CN）字段。仅在启用了 mTLS 且客户端提供了有效证书时有值；否则返回 `nil`。

**返回值**：客户端证书 Common Name 字符串，或 `nil`

**示例**：
```
let cn = req.client_cert_cn
if cn == nil {
  return deny(403, "client certificate required")
}
if cn != "trusted-service" {
  return deny(403, "unauthorized client: " + cn)
}
```

---

### req.client_cert_fingerprint

**签名**：`req.client_cert_fingerprint` → `Str | Nil`

读取 mTLS 客户端证书的指纹哈希值（十六进制字符串）。仅在启用了 mTLS 且客户端提供了有效证书时有值；否则返回 `nil`。

**返回值**：证书指纹哈希字符串，或 `nil`

**示例**：
```
let fp = req.client_cert_fingerprint
if fp != nil {
  ctx.set("client_cert_fp", fp)
}
```

---

## 请求修改（req.*）

### req.set_header

**签名**：`req.set_header(name, value)` → `Nil`

设置或覆盖请求 Header。若该 Header 已存在，原有值被替换；若不存在，则新增。该修改对上游可见，但对客户端不可见。

**参数**：
- `name`（Str）— Header 名称
- `value`（Str）— Header 值

**返回值**：`nil`

**示例**：
```
req.set_header("X-Request-Id", uuid())
req.set_header("X-Forwarded-User", "alice")
```

---

### req.append_header

**签名**：`req.append_header(name, value)` → `Nil`

向请求中追加一个 Header。不会覆盖已有的同名 Header，而是追加一个新的同名条目。

**参数**：
- `name`（Str）— Header 名称
- `value`（Str）— 要追加的 Header 值

**返回值**：`nil`

**示例**：
```
req.append_header("X-Tags", "internal")
req.append_header("X-Tags", "verified")
```

---

### req.remove_header

**签名**：`req.remove_header(name)` → `Nil`

删除请求中指定名称的 Header（删除所有同名条目）。若 Header 不存在，操作无副作用。

**参数**：
- `name`（Str）— 要删除的 Header 名称

**返回值**：`nil`

**示例**：
```
req.remove_header("X-Internal-Debug")
req.remove_header("Cookie")
```

---

### req.set_uri

**签名**：`req.set_uri(uri)` → `Nil`

重写转发给上游的请求 URI。`uri` 应包含路径，可选地包含查询字符串（格式为 `/path?key=value`）。Host 和协议部分不受影响。

**参数**：
- `uri`（Str）— 新的 URI，必须以 `/` 开头

**返回值**：`nil`

**示例**：
```
let path = req.path
let new_path = regex_replace(path, "^/v1/", "/v2/")
req.set_uri(new_path)
```

---

### req.set_host

**签名**：`req.set_host(host)` → `Nil`

重写请求的 `Host` Header，影响转发至上游的 Host 值。

**参数**：
- `host`（Str）— 新的 Host 值，例如 `"backend.internal:8080"`

**返回值**：`nil`

**示例**：
```
req.set_host("internal-api.svc.cluster.local")
```

---

### req.set_method

**签名**：`req.set_method(method)` → `Nil`

修改请求的 HTTP 方法。传入的方法名称不区分大小写，底层会转换为大写后应用。

**参数**：
- `method`（Str）— 新的 HTTP 方法，例如 `"GET"`、`"POST"`

**返回值**：`nil`

**示例**：
```
if req.method == "HEAD" {
  req.set_method("GET")
}
```

---

## 响应读取（resp.*）

> **注意**：以下函数仅在响应阶段（`upstreamResponseFilterPlugins`）有效。在请求阶段调用时，读取函数通常返回 `nil`，状态码返回 `nil`。

### resp.status

**签名**：`resp.status` → `Int | Nil`

读取上游返回的响应状态码。仅在响应阶段有值；在请求阶段（无上游响应）返回 `nil`。

**返回值**：HTTP 状态码整数，或 `nil`

**示例**：
```
let status = resp.status
if status != nil and status >= 500 {
  resp.set_header("X-Upstream-Error", "true")
}
```

---

### resp.header

**签名**：`resp.header(name)` → `Str | Nil`

读取上游响应中指定名称的 Header 值。若不存在，返回 `nil`。

**参数**：
- `name`（Str）— Header 名称，不区分大小写

**返回值**：Header 值字符串，或 `nil`

**示例**：
```
let cache = resp.header("Cache-Control")
if cache == nil {
  resp.set_header("Cache-Control", "no-store")
}
```

---

### resp.has_header

**签名**：`resp.has_header(name)` → `Bool`

检查上游响应是否包含指定名称的 Header。

**参数**：
- `name`（Str）— Header 名称

**返回值**：`true` 表示 Header 存在，`false` 表示不存在

**示例**：
```
if not resp.has_header("X-Request-Id") {
  let req_id = ctx.get("request_id")
  if req_id != nil {
    resp.set_header("X-Request-Id", req_id)
  }
}
```

---

### resp.header_names

**签名**：`resp.header_names` → `List`

返回上游响应中所有 Header 名称的列表。最多返回 256 个名称，超出部分被截断。

**返回值**：包含所有响应 Header 名称的 `List`

**限制**：最多返回 256 个 Header 名称

**示例**：
```
let names = resp.header_names
for name in names {
  if name == "X-Internal-Header" {
    resp.remove_header(name)
  }
}
```

---

### resp.content_type

**签名**：`resp.content_type` → `Str | Nil`

读取上游响应的 `Content-Type` Header 值。若不存在，返回 `nil`。等价于 `resp.header("Content-Type")`。

**返回值**：Content-Type 字符串，或 `nil`

**示例**：
```
let ct = resp.content_type
if ct != nil and regex_find(ct, "text/html") != nil {
  resp.set_header("X-Content-Type-Options", "nosniff")
}
```

---

## 响应修改（resp.*）

### resp.set_status

**签名**：`resp.set_status(code)` → `Nil`

修改发送给客户端的响应状态码。`code` 必须为 100 到 599 之间的整数，否则会触发运行时错误。

**参数**：
- `code`（Int）— HTTP 状态码，范围 100–599

**返回值**：`nil`

**限制**：`code` 必须为整数类型，且在 100–599 范围内；不在范围内时抛出运行时错误。

**示例**：
```
let status = resp.status
if status == 404 {
  resp.set_status(200)
  resp.set_header("X-Redirected", "true")
}
```

---

### resp.set_header

**签名**：`resp.set_header(name, value)` → `Nil`

设置或覆盖响应 Header。若该 Header 已存在，原有值被替换；若不存在，则新增。该修改对客户端可见。

**参数**：
- `name`（Str）— Header 名称
- `value`（Str）— Header 值

**返回值**：`nil`

**示例**：
```
resp.set_header("X-Powered-By", "Edgion")
resp.set_header("Strict-Transport-Security", "max-age=31536000; includeSubDomains")
```

---

### resp.append_header

**签名**：`resp.append_header(name, value)` → `Nil`

向响应中追加一个 Header，不覆盖已有同名 Header。

**参数**：
- `name`（Str）— Header 名称
- `value`（Str）— 要追加的值

**返回值**：`nil`

**示例**：
```
resp.append_header("Vary", "Accept-Encoding")
resp.append_header("Vary", "Accept-Language")
```

---

### resp.remove_header

**签名**：`resp.remove_header(name)` → `Nil`

删除响应中指定名称的所有同名 Header 条目。若 Header 不存在，操作无副作用。

**参数**：
- `name`（Str）— 要删除的 Header 名称

**返回值**：`nil`

**示例**：
```
resp.remove_header("Server")
resp.remove_header("X-Powered-By")
```

---

## 上下文存储（ctx.*）

上下文变量在同一请求生命周期内跨插件共享。请求阶段设置的值在响应阶段和访问日志中均可读取。

### ctx.get

**签名**：`ctx.get(key)` → `Str | Nil`

读取当前请求上下文中指定键的值。若该键未设置，返回 `nil`。

**参数**：
- `key`（Str）— 上下文变量键名

**返回值**：值字符串，或 `nil`

**示例**：
```
let user = ctx.get("authenticated_user")
if user != nil {
  req.set_header("X-User", user)
}
```

---

### ctx.set

**签名**：`ctx.set(key, value)` → `Nil`

在当前请求上下文中设置一个键值对。该值对同一请求的后续插件和访问日志可见。`value` 会被转换为字符串存储。

**参数**：
- `key`（Str）— 上下文变量键名
- `value`（Str）— 要存储的值

**返回值**：`nil`

**示例**：
```
let user_id = req.path_param("userId")
ctx.set("user_id", user_id)
ctx.set("request_start", to_str(time_now()))
```

---

### ctx.remove

**签名**：`ctx.remove(key)` → `Nil`

删除上下文中指定键的变量。若键不存在，操作无副作用。

**参数**：
- `key`（Str）— 要删除的键名

**返回值**：`nil`

**示例**：
```
ctx.remove("temp_flag")
```

---

## 字符串处理

### len

**签名**：`len(s)` → `Int`

返回字符串的 Unicode 字符数（非字节数），或列表的元素个数。对 `nil` 返回 `0`。

**参数**：
- `s`（Str | Nil | List）— 输入值

**返回值**：字符数（对字符串）或元素数（对列表），`nil` 返回 `0`

**限制**：参数类型仅支持 `Str`、`Nil`、`List`，其他类型会触发类型错误。

> **注意（与 `max_string_len` 单位不一致）**：`len()` 返回的是 **Unicode 字符数**，而沙箱的 `max_string_len` 限制（默认 8192）以及相应的 `string too long (N bytes > L bytes)` 错误信息都是按 **UTF-8 字节** 度量的。对 ASCII 字符串两者相等；对含多字节字符（如中文、emoji）的字符串，字节长度最多可达字符数的 4 倍，因此 `len(s) < max_string_len` 这样的脚本守卫并不能保证后续的字符串拼接或内置函数调用一定能通过字节长度检查。

**示例**：
```
let token = req.header("X-Api-Key")
if token == nil or len(token) < 32 {
  return deny(401, "invalid API key length")
}
```

---

### substr

**签名**：`substr(s, start, end)` → `Str`

返回字符串的子字符串，按 Unicode 字符索引切片（0 起始，左闭右开区间）。索引越界时自动截断到字符串边界，不会报错。对 `nil` 输入返回空字符串。

**参数**：
- `s`（Str | Nil）— 源字符串
- `start`（Int）— 起始字符索引（含），0 起始
- `end`（Int）— 结束字符索引（不含），0 起始

**返回值**：子字符串

**示例**：
```
let auth = req.header("Authorization")
if auth != nil and len(auth) > 7 {
  let scheme = substr(auth, 0, 7)
  if to_upper(scheme) == "BEARER " {
    let token = substr(auth, 7, len(auth))
    req.set_header("X-Token", token)
  }
}
```

---

### to_str

**签名**：`to_str(v)` → `Str`

将任意类型转换为字符串：
- `Str` → 原值
- `Int` → 十进制表示
- `Bool` → `"true"` 或 `"false"`
- `Nil` → `"nil"`
- `List` → `"[list]"`

**参数**：
- `v`（任意类型）— 输入值

**返回值**：字符串表示

**示例**：
```
let code = resp.status
if code != nil {
  resp.set_header("X-Status-Code", to_str(code))
}
```

---

### to_int

**签名**：`to_int(s)` → `Int | Nil`

将字符串解析为 64 位整数。若输入已是 `Int`，直接返回。解析失败或输入为其他类型时返回 `nil`。

**参数**：
- `s`（Str | Int）— 要解析的值

**返回值**：整数值，或 `nil`（解析失败）

**示例**：
```
let limit_str = req.header("X-Rate-Limit")
let limit = to_int(limit_str)
if limit == nil or limit <= 0 {
  limit = 100
}
```

---

### to_upper

**签名**：`to_upper(s)` → `Str`

返回字符串的大写形式。对非 ASCII 字符也按 Unicode 规则转换。

**参数**：
- `s`（Str）— 输入字符串

**返回值**：大写字符串

**示例**：
```
let method = to_upper(req.method)
if method != "GET" and method != "POST" {
  return deny(405, "only GET/POST allowed")
}
```

---

### to_lower

**签名**：`to_lower(s)` → `Str`

返回字符串的小写形式。

**参数**：
- `s`（Str）— 输入字符串

**返回值**：小写字符串

**示例**：
```
let ct = req.content_type
if ct != nil and to_lower(ct) == "application/json" {
  req.set_header("X-Is-Json", "true")
}
```

---

### trim

**签名**：`trim(s)` → `Str`

去除字符串首尾的 ASCII 空白字符（空格、制表符、换行等）。

**参数**：
- `s`（Str）— 输入字符串

**返回值**：去除首尾空白后的字符串

**示例**：
```
let key = req.header("X-Api-Key")
if key != nil {
  key = trim(key)
  if len(key) == 0 {
    return deny(401, "empty API key")
  }
}
```

---

### split

**签名**：`split(s, delimiter)` → `List`

按分隔符将字符串切分为列表。若分隔符为空字符串，将字符串切分为单字符列表。

**参数**：
- `s`（Str）— 要切分的字符串
- `delimiter`（Str）— 分隔符

**返回值**：字符串列表（`List`）

**示例**：
```
let roles_header = req.header("X-Roles")
if roles_header != nil {
  let roles = split(roles_header, ",")
  for role in roles {
    if trim(role) == "admin" {
      req.set_header("X-Is-Admin", "true")
    }
  }
}
```

---

### join

**签名**：`join(list, delimiter)` → `Str`

将列表中的所有元素用分隔符拼接为一个字符串。

**参数**：
- `list`（List）— 要拼接的字符串列表
- `delimiter`（Str）— 元素间的分隔符

**返回值**：拼接后的字符串

**限制**：第一个参数必须为 `List` 类型，否则触发类型错误。

**示例**：
```
let parts = split(req.path, "/")
let rejoined = join(parts, "-")
req.set_header("X-Path-Slug", rejoined)
```

---

## 编码与解码

### base64_encode

**签名**：`base64_encode(s)` → `Str`

将字符串进行标准 Base64 编码（含填充字符 `=`）。

**参数**：
- `s`（Str）— 要编码的字符串

**返回值**：Base64 编码字符串

**示例**：
```
let credentials = "user:password"
let encoded = base64_encode(credentials)
req.set_header("Authorization", "Basic " + encoded)
```

---

### base64_decode

**签名**：`base64_decode(s)` → `Str | Nil`

将 Base64 编码字符串解码为原始字符串。若输入不是合法的 Base64 编码，返回 `nil`。解码结果以 UTF-8 解释，非法字节将被替换为 Unicode 替换字符。

**参数**：
- `s`（Str）— Base64 编码字符串

**返回值**：解码后的字符串，或 `nil`（解码失败）

**示例**：
```
let auth = req.header("Authorization")
if auth != nil and len(auth) > 6 {
  let encoded = substr(auth, 6, len(auth))
  let decoded = base64_decode(encoded)
  if decoded != nil {
    let parts = split(decoded, ":")
    req.set_header("X-Username", parts[0])
  }
}
```

---

### url_encode

**签名**：`url_encode(s)` → `Str`

对字符串进行 URL 百分比编码（Percent Encoding）。字母、数字及 `-`、`_`、`.`、`~` 不编码，其余字符编码为 `%XX` 形式（大写十六进制）。

**参数**：
- `s`（Str）— 要编码的字符串

**返回值**：URL 编码后的字符串

**示例**：
```
let value = req.query("redirect")
if value != nil {
  let safe = url_encode(value)
  req.set_header("X-Redirect-Encoded", safe)
}
```

---

### url_decode

**签名**：`url_decode(s)` → `Str`

按 `application/x-www-form-urlencoded` 语义对 URL 编码字符串进行解码：`%XX` 序列被还原为对应字符，**`+` 被还原为空格**。不合法的 `%XX` 序列会被原样保留。

> **语义说明**：本内建采用表单体 / 查询字符串解码语义，**不是** RFC 3986 路径解码。RFC 3986 中路径里的字面量 `+` 是保留字符，必须原样保留；本内建会把它当作空格。这一行为与路由匹配和一致性哈希参数名比较所使用的 `HttpRouteRuleUnit::url_decode` 保持一致，使 DSL 与路由层在 query 数据上得到相同的解码结果。
>
> **安全告警 — 不要用于 URL 路径的 ACL / 白名单比较。** 如果你调用 `url_decode(req.path())` 并与含字面量 `+` 的白名单比较，攻击者可以用 `%20`（或反过来）替换，使两个不同的路径被判定相等。在安全场景中做路径归一化，应直接对 `req.path()`（已由 pingora 归一化）做比较，或使用不会把 `+` 与空格折叠的字符串操作（`starts_with`、`regex_match`）。当前 DSL 不提供 RFC 3986 路径解码内建；推荐的路径 ACL 模式是针对原始（已归一化）路径写规则。

**参数**：
- `s`（Str）— URL 编码字符串（适用于查询字符串或表单体）

**返回值**：解码后的字符串

**示例**（查询字符串解码 — 推荐用途）：
```
let raw = req.query("callback")
if raw != nil {
  let decoded = url_decode(raw)
  req.set_header("X-Callback-Url", decoded)
}
```

---

### hex_encode

**签名**：`hex_encode(s)` → `Str`

将字符串的每个字节编码为两位小写十六进制字符。

**参数**：
- `s`（Str）— 要编码的字符串

**返回值**：十六进制字符串（小写）

**示例**：
```
let raw = req.header("X-Binary-Data")
if raw != nil {
  let encoded = hex_encode(raw)
  log("binary data as hex: " + encoded)
}
```

---

### hex_decode

**签名**：`hex_decode(s)` → `Str | Nil`

将十六进制字符串解码为原始字节串（以 UTF-8 解释）。若输入包含非法十六进制字符，返回 `nil`。

**参数**：
- `s`（Str）— 十六进制字符串（不区分大小写）

**返回值**：解码后的字符串，或 `nil`（输入不合法）

**示例**：
```
let hex_token = req.header("X-Hex-Token")
if hex_token != nil {
  let raw = hex_decode(hex_token)
  if raw == nil {
    return deny(400, "invalid hex token")
  }
}
```

---

## 哈希与加密

### sha256

**签名**：`sha256(s)` → `Str`

计算字符串的 SHA-256 哈希值，返回 64 字符的小写十六进制摘要。

**参数**：
- `s`（Str）— 输入字符串

**返回值**：64 字符十六进制字符串

**示例**：
```
let key = req.header("X-Api-Key")
if key != nil {
  let hashed = sha256(key)
  req.set_header("X-Api-Key-Hash", hashed)
  req.remove_header("X-Api-Key")
}
```

---

### md5

**签名**：`md5(s)` → `Str`

计算字符串的 MD5 哈希值，返回 32 字符的小写十六进制摘要。注意 MD5 不适合安全场景（如密码存储），仅适合用于数据完整性校验或缓存键生成等非安全目的。

**参数**：
- `s`（Str）— 输入字符串

**返回值**：32 字符十六进制字符串

**示例**：
```
let body_hash = md5(req.path + to_str(time_now()))
req.set_header("X-Cache-Key", body_hash)
```

---

### hmac_sha256

**签名**：`hmac_sha256(key, data)` → `Str`

使用 HMAC-SHA256 算法计算消息认证码，返回 64 字符的小写十六进制字符串。常用于验证请求签名。

**参数**：
- `key`（Str）— 密钥字符串
- `data`（Str）— 要签名的数据

**返回值**：64 字符十六进制 HMAC 摘要

**示例**：
```
let secret = "my-secret-key"
let timestamp = req.header("X-Timestamp")
let signature = req.header("X-Signature")
if timestamp == nil or signature == nil {
  return deny(401, "missing signature headers")
}
let expected = hmac_sha256(secret, timestamp + ":" + req.path)
if signature != expected {
  return deny(401, "invalid signature")
}
```

---

## 正则表达式

### regex_find

**签名**：`regex_find(s, pattern)` → `Str | Nil`

在字符串中查找正则表达式的第一个匹配项，返回匹配的子字符串。若不匹配，返回 `nil`。正则表达式语法使用 Rust regex 库语法（RE2 风格，不支持回溯）。

**参数**：
- `s`（Str）— 被搜索的字符串
- `pattern`（Str）— 正则表达式模式

**返回值**：第一个匹配的子字符串，或 `nil`

**限制**：pattern 必须为字符串字面量。1024 字节长度限制和正则语法均在编译期校验。

**示例**：
```
let ct = req.content_type
if ct != nil {
  let mime = regex_find(ct, "^[^;]+")
  if mime != nil {
    req.set_header("X-Mime-Type", trim(mime))
  }
}
```

---

### regex_replace

**签名**：`regex_replace(s, pattern, replacement)` → `Str`

将字符串中所有匹配正则表达式的部分替换为指定字符串。支持捕获组引用（如 `$1`、`$2`）。

**参数**：
- `s`（Str）— 原字符串
- `pattern`（Str）— 正则表达式模式
- `replacement`（Str）— 替换字符串，可使用 `$1` 等捕获组引用

**返回值**：替换后的字符串

**限制**：pattern 必须为字符串字面量。1024 字节长度限制和正则语法均在编译期校验。

**示例**：
```
let path = req.path
let new_path = regex_replace(path, "^/api/v1/", "/api/v2/")
req.set_uri(new_path)
```

---

## 时间

### time_now

**签名**：`time_now()` → `Int`

返回当前时间的 Unix 时间戳（自 1970-01-01 00:00:00 UTC 起的秒数）。

**返回值**：Unix 时间戳（整数，单位：秒）

**示例**：
```
let ts = time_now()
let request_ts = to_int(req.header("X-Timestamp"))
if request_ts == nil or ts - request_ts > 300 {
  return deny(401, "request expired")
}
```

---

### time_format

**签名**：`time_format(timestamp, format)` → `Str`

将 Unix 时间戳格式化为字符串。格式字符串使用 `chrono` 库的格式规范（类似 C 语言 `strftime`）。若时间戳无效，返回 `nil`。

**参数**：
- `timestamp`（Int）— Unix 时间戳（秒）
- `format`（Str）— chrono 格式字符串，例如 `"%Y-%m-%dT%H:%M:%SZ"`

**返回值**：格式化后的时间字符串

**限制**：格式字符串最大长度为 256 字节；超出时触发运行时错误。无效的格式说明符（例如未知的 `%X`）同样会触发运行时错误，而不是返回字符串。

**常用格式字符**：
- `%Y` — 4 位年份
- `%m` — 2 位月份（01–12）
- `%d` — 2 位日期（01–31）
- `%H` — 2 位小时（00–23）
- `%M` — 2 位分钟（00–59）
- `%S` — 2 位秒（00–59）

**示例**：
```
let ts = time_now()
let date_str = time_format(ts, "%Y-%m-%dT%H:%M:%SZ")
req.set_header("X-Request-Time", date_str)
```

---

## JSON

### json_get

**签名**：`json_get(json_str, path)` → `Str | Int | Bool | Nil`

从 JSON 字符串中按点分隔路径提取值。路径格式为 `"field"` 或 `"parent.child.grandchild"`；对数组可用数字索引，如 `"items.0.name"`。若路径不存在或 JSON 解析失败，返回 `nil`。

**参数**：
- `json_str`（Str）— JSON 字符串
- `path`（Str）— 点分隔的字段路径

**返回值**：提取的值（`Str`、`Int`、`Bool` 或 `Nil`）

**限制**：
- JSON 字符串长度不得超过 `maxStringLen`（默认 8192 字节）
- 路径最多支持 32 段，超出时触发运行时错误
- JSON 数字若可表示为 `i64` 则返回 `Int`，否则返回 `Str`；数组/对象节点返回其 JSON 字符串表示

**示例**：
```
let payload = req.header("X-Jwt-Claims")
if payload != nil {
  let user_id = json_get(payload, "sub")
  let role = json_get(payload, "role")
  if user_id != nil {
    req.set_header("X-User-Id", to_str(user_id))
  }
  if role == "admin" {
    req.set_header("X-Is-Admin", "true")
  }
}
```

---

## 网络

### http.call

**签名**：`http.call(method, url, body [, timeout_ms [, headers]])` → `Map`

从网关向任意 HTTP/HTTPS 端点发起出站请求。从脚本角度看，该调用是同步的，收到响应或超时后返回。

**参数**：
- `method`（Str）— HTTP 方法，如 `"GET"`、`"POST"`、`"PUT"`
- `url`（Str）— 包含协议和主机名的完整 URL，如 `"https://api.example.com/v1/check"`
- `body`（Str | Nil）— 请求体；不需要请求体时传 `nil`
- `timeout_ms`（Int，可选）— 单次调用超时（毫秒）；上限为 `httpMaxTimeoutMs`（默认 5000）
- `headers`（Map，可选）— 附加请求头，以字符串键值对 `Map` 表示

**返回值**：包含以下字段的 `Map`：

| 字段 | 类型 | 说明 |
|------|------|------|
| `status` | Int | HTTP 响应状态码；调用失败（超时、连接错误、SSRF 拦截等）时为 `0` |
| `body` | Str | UTF-8 响应体；失败或响应体为空时为空字符串 |
| `headers` | Map | 响应头；键为小写头名称，值为该头名称对应的最后一个值 |

**此函数永远不会抛出运行时错误。** 网络故障、超时、SSRF 拦截均返回 `status = 0` 和空 body。

**限制**：
- 仅接受 `http` 和 `https` 协议；其他协议（如 `file://`、`ftp://`）返回 `status = 0`。
- 当 `httpBlockPrivateIps: true` 时，URL host 为回环、私有、链路本地 **IP 字面量**的请求返回 `status = 0`。该守卫**仅检查 URL host 字符串，不解析 DNS 域名**，因此解析到私有地址的域名不会被此开关拦截。域名形式的 SSRF 防护应以网络层出站控制（NetworkPolicy）为主。
- 响应体截断在 `min(httpMaxResponseBodyBytes, max_string_len)`（默认分别为 1 MiB 与 8 KiB）。两者中较小者生效，保证返回给脚本的 body 与其它 DSL 字符串值共享同一长度上限。截断为静默行为——调用仍成功，且依然不会抛出运行时错误。
- 响应 `headers` Map 超过 `max_map_size` 时静默截断为前 `max_map_size` 项（SSRF / 大响应头防护）；调用仍返回成功。
- 实际超时为 `min(timeout_ms, httpMaxTimeoutMs)`。
- 全局客户端最多跟随 10 次重定向。当 `httpBlockPrivateIps: true` 时，同一 IP 字面量守卫会对**每一跳重定向**重新应用，因此跳到被阻断地址的 30x 会得到 `status = 0`，而非静默跟随。

**配置字段**（位于插件 `config` 块内）：

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `httpMaxTimeoutMs` | Integer | `5000` | 单次调用超时上限（毫秒） |
| `httpMaxResponseBodyBytes` | Integer | `1048576` | 响应体最大字节数；超出时截断 |
| `httpBlockPrivateIps` | Boolean | `false` | 拦截对私有/回环/链路本地 IP 的出站调用（SSRF 防护）。仅检查 IP 字面量，不解析域名（见上方说明）。 |

**示例 — 调用健康检查端点并根据状态分支**：
```
let r = http.call("GET", "http://internal-service/health", nil, 1000)
if r["status"] == 200 {
  req.set_header("X-Backend-Healthy", "true")
} else {
  return deny(503, "backend unavailable")
}
```

**示例 — POST JSON 并携带自定义请求头**：
```
let hdrs = {"Content-Type": "application/json", "X-Api-Key": req.header("X-Api-Key")}
let r = http.call("POST", "https://auth.example.com/verify", "{\"token\":\"abc\"}", 2000, hdrs)
if r["status"] != 200 {
  return deny(401, "auth check failed")
}
```

**示例 — 读取响应头**：
```
let r = http.call("GET", "https://api.example.com/quota", nil, 2000)
let remaining = r["headers"]["x-ratelimit-remaining"]
if remaining != nil {
  resp.set_header("X-Quota-Remaining", remaining)
}
```

---

## Redis（redis.*）

所有 `redis.*` 函数均由预先配置的 LinkSys Redis 连接池提供支持。连接池通过 `linkSysRefs` 配置字段在请求时解析。每个 `redis.*` 调用都需要 `link`（别名）作为第一个参数，用于识别使用哪个连接。

**错误协议**：每个 `redis.*` 函数均返回 `Map { ok: Bool, value: <类型>, error: Str | Nil }`。函数**不会触发运行时错误**，也**不会 panic**。脚本作者自行决定如何处理失败（快速拒绝、降级放行或优雅退化）。

- `ok: true` — 操作成功；`value` 为结果值；`error` 为 `nil`。
- `ok: false` — Redis 连接或操作出错；`value` 为 `nil`；`error` 为错误信息。
- 当 `link` 参数不匹配 `linkSysRefs` 中的任何别名时，返回 `ok: false, error: "link 'x' not found in linkSysRefs"`。
- 当指定的 link 存在但尚未就绪时，返回 `ok: false, error: "link 'x' not ready"`。

**配置字段**（位于 `config` 内部）：

| 字段 | 类型 | 是否必填 | 说明 |
|------|------|----------|------|
| `linkSysRefs` | Vec<{namespace, name}> | 使用 `redis.*` 时必填 | LinkSys Redis 资源列表。每个元素包含 `namespace` 和 `name` 字段；`name` 是用于 `redis.*` 调用的别名。若脚本中存在任何 `redis.*` 内置调用，此字段为必填项；缺失时插件加载失败。 |

### redis.get

**签名**：`redis.get(link, key)` → `Map { ok: Bool, value: Str | Nil, error: Str | Nil }`

读取字符串键的值。键不存在时返回 `value: nil`。

**参数**：
- `link`（Str）— 来自 `linkSysRefs[].name` 的别名
- `key`（Str）— Redis 键名

**示例 — 快速拒绝**：
```
let r = redis.get("cache", "feature:dark-mode")
if !r["ok"] {
  log_warn("redis error: " + r["error"])
  return deny(503, "service unavailable")
}
if r["value"] == "on" {
  req.set_header("X-Feature-Dark-Mode", "true")
}
```

---

### redis.set

**签名**：`redis.set(link, key, value)` 或 `redis.set(link, key, value, ttl_ms)` → `Map { ok: Bool, value: Bool, error: Str | Nil }`

设置字符串键的值，可选传入毫秒级 TTL。

**参数**：
- `link`（Str）— 来自 `linkSysRefs[].name` 的别名
- `key`（Str）— Redis 键名
- `value`（Str）— 存储的值
- `ttl_ms`（Int，可选）— 过期时间（毫秒）；`nil` 或 `0` 表示不设置过期

---

### redis.del

**签名**：`redis.del(link, key)` → `Map { ok: Bool, value: Bool, error: Str | Nil }`

删除一个键。键被删除时返回 `value: true`，键不存在时返回 `value: false`。

**参数**：
- `link`（Str）— 来自 `linkSysRefs[].name` 的别名
- `key`（Str）— Redis 键名

---

### redis.exists

**签名**：`redis.exists(link, key)` → `Map { ok: Bool, value: Bool, error: Str | Nil }`

检查键是否存在。

**参数**：
- `link`（Str）— 来自 `linkSysRefs[].name` 的别名
- `key`（Str）— Redis 键名

---

### redis.ttl

**签名**：`redis.ttl(link, key)` → `Map { ok: Bool, value: Int, error: Str | Nil }`

返回键的剩余生存时间（秒）。

| `value` | 含义 |
|---------|------|
| `>= 0` | 剩余 TTL（秒） |
| `-1` | 键存在但无过期时间 |
| `-2` | 键不存在 |

仅在连接错误时才返回 `ok: false`。

**参数**：
- `link`（Str）— 来自 `linkSysRefs[].name` 的别名
- `key`（Str）— Redis 键名

---

### redis.expire

**签名**：`redis.expire(link, key, seconds)` → `Map { ok: Bool, value: Bool, error: Str | Nil }`

为已有键设置过期时间（秒）。若键不存在返回 `value: false`。

**参数**：
- `link`（Str）— 来自 `linkSysRefs[].name` 的别名
- `key`（Str）— Redis 键名
- `seconds`（Int）— 过期时间（秒）

---

### redis.incr

**签名**：`redis.incr(link, key)` → `Map { ok: Bool, value: Int, error: Str | Nil }`

将键的整数值原子自增 1。键不存在时创建并置为 1。

**参数**：
- `link`（Str）— 来自 `linkSysRefs[].name` 的别名
- `key`（Str）— Redis 键名

**示例 — 简单限流**：
```
let r = redis.incr("cache", "rate:" + req.client_ip)
if !r["ok"] {
  return deny(503, "service unavailable")
}
if r["value"] > 100 {
  return deny(429, "rate limit exceeded")
}
```

---

### redis.incr_by

**签名**：`redis.incr_by(link, key, delta)` → `Map { ok: Bool, value: Int, error: Str | Nil }`

将键的整数值原子增加 `delta`（可为负数）。

**参数**：
- `link`（Str）— 来自 `linkSysRefs[].name` 的别名
- `key`（Str）— Redis 键名
- `delta`（Int）— 增加量（可为负数）

---

### redis.hget

**签名**：`redis.hget(link, key, field)` → `Map { ok: Bool, value: Str | Nil, error: Str | Nil }`

获取哈希字段的值。键或字段不存在时返回 `value: nil`。

**参数**：
- `link`（Str）— 来自 `linkSysRefs[].name` 的别名
- `key`（Str）— 哈希键名
- `field`（Str）— 字段名

---

### redis.hset

**签名**：`redis.hset(link, key, field, value)` → `Map { ok: Bool, value: Bool, error: Str | Nil }`

设置哈希字段的值。成功时返回 `value: true`。

**参数**：
- `link`（Str）— 来自 `linkSysRefs[].name` 的别名
- `key`（Str）— 哈希键名
- `field`（Str）— 字段名
- `value`（Str）— 字段值

---

### redis.hdel

**签名**：`redis.hdel(link, key, field)` → `Map { ok: Bool, value: Bool, error: Str | Nil }`

删除哈希字段。字段存在并被删除时返回 `value: true`，否则返回 `value: false`。

**参数**：
- `link`（Str）— 来自 `linkSysRefs[].name` 的别名
- `key`（Str）— 哈希键名
- `field`（Str）— 字段名

---

### redis.hgetall

**签名**：`redis.hgetall(link, key)` → `Map { ok: Bool, value: Map, error: Str | Nil }`

获取哈希的所有字段和值。键不存在时返回空 `Map`。在连接错误,或哈希条目数超过 `max_map_size` 限制时返回 `ok: false`(后者不会构造超大 `Map`,`error` 会描述实际大小与上限)。

**参数**：
- `link`（Str）— 来自 `linkSysRefs[].name` 的别名
- `key`（Str）— 哈希键名

**示例**：
```
let r = redis.hgetall("cache", "user:config:1234")
if r["ok"] {
  let theme = r["value"]["theme"]
  if theme != nil {
    resp.set_header("X-User-Theme", theme)
  }
}
```

---

### redis.hexists

**签名**：`redis.hexists(link, key, field)` → `Map { ok: Bool, value: Bool, error: Str | Nil }`

检查哈希字段是否存在。

**参数**：
- `link`（Str）— 来自 `linkSysRefs[].name` 的别名
- `key`（Str）— 哈希键名
- `field`（Str）— 字段名

---

### ip_in_cidr

**签名**：`ip_in_cidr(ip, cidr)` → `Bool`

检查指定 IP 地址是否属于给定的 CIDR 网段。同时支持 IPv4 和 IPv6。IPv4 与 IPv6 地址互相比较时始终返回 `false`。若 IP 地址或 CIDR 格式不合法，返回 `false`（不触发错误）。

**参数**：
- `ip`（Str）— IP 地址字符串，如 `"192.168.1.100"` 或 `"::1"`
- `cidr`（Str）— CIDR 网段字符串，如 `"192.168.0.0/16"` 或 `"fc00::/7"`

**返回值**：`true` 表示 IP 在网段内，`false` 表示不在或格式错误

**示例**：
```
let ip = req.client_ip
if ip_in_cidr(ip, "10.0.0.0/8") or ip_in_cidr(ip, "172.16.0.0/12") or ip_in_cidr(ip, "192.168.0.0/16") {
  req.set_header("X-Private-Network", "true")
} else {
  return deny(403, "access from public networks is not allowed")
}
```

---

## 控制流函数

以下函数用于终止当前请求的处理流程，属于脚本的特殊返回语句，通常与 `return` 关键字结合使用。

### deny

**签名**：`deny(status, body)` — 终止请求

向客户端返回指定的 HTTP 错误响应并终止处理。`status` 为超出范围的整数时，自动降级为 500。`body` 为 `nil` 时响应体为空。

**参数**：
- `status`（Int）— HTTP 状态码，范围 100–599
- `body`（Str | Nil）— 响应体字符串，传 `nil` 则无响应体

**示例**：
```
let token = req.header("X-Api-Key")
if token == nil {
  return deny(401, "API key required")
}
if len(token) < 16 {
  return deny(400, "API key too short")
}
```

---

### grpc_error

**签名**：`grpc_error(code, message)` — 终止请求（仅请求阶段）

向客户端返回标准 gRPC 错误，通过 HTTP/2 trailers 发送 `grpc-status` 和 `grpc-message`。`code` 为 0–16 范围的 gRPC 状态码，超出范围时自动降级为 13（INTERNAL）。

**参数**：
- `code`（Int）— gRPC 状态码（0–16）
- `message`（Str | Nil）— 错误消息，传 `nil` 则无消息

**限制**：仅在请求阶段（`requestPlugins`）有效。在响应阶段调用会降级为 `terminate()`，因为响应头已提交，无法再追加 HTTP/2 trailers。

**常用 gRPC 状态码**：
- `0` — OK
- `1` — CANCELLED
- `2` — UNKNOWN
- `3` — INVALID_ARGUMENT
- `5` — NOT_FOUND
- `7` — PERMISSION_DENIED
- `13` — INTERNAL
- `16` — UNAUTHENTICATED

**示例**：
```
if req.protocol == "grpc" {
  let token = req.header("Authorization")
  if token == nil {
    return grpc_error(16, "unauthenticated: missing token")
  }
}
```

---

### terminate

**签名**：`terminate()` — 终止请求

直接终止请求处理，不向客户端发送任何响应（连接被关闭）。通常不直接使用，主要作为 `grpc_error()` 在响应阶段的降级行为。

**示例**：
```
return terminate()
```

---

## 其他工具

### uuid

**签名**：`uuid()` → `Str`

生成一个随机的 UUID v4，格式为标准的 8-4-4-4-12 十六进制字符串，例如 `"550e8400-e29b-41d4-a716-446655440000"`。

**返回值**：UUID v4 字符串（36 字符，含连字符）

**示例**：
```
if not req.has_header("X-Request-Id") {
  req.set_header("X-Request-Id", uuid())
}
```

---

### range

**签名**：`range(start, end)` → `List`

生成一个从 `start`（含）到 `end`（不含）的整数序列，元素为数字字符串形式。

**参数**：
- `start`（Int）— 起始值（含）
- `end`（Int）— 结束值（不含）

**返回值**：字符串数字列表

**限制**：最多生成 1000 个元素，超出部分被截断（即 `end - start > 1000` 时只取前 1000 个）。

**示例**：
```
let nums = range(1, 6)
let joined = join(nums, ",")
req.set_header("X-Page-Range", joined)
```

---

### log

**签名**：`log(msg)` → `Nil`

以 INFO 级别记录一条日志消息。日志内容会附加到当前请求的 DSL 执行日志中，并可通过访问日志的 `dsl_log` 字段查看。

**参数**：
- `msg`（Str）— 日志消息

**返回值**：`nil`

**示例**：
```
let user = req.header("X-User-Id")
log("processing request for user: " + to_str(user))
```

---

### log_warn

**签名**：`log_warn(msg)` → `Nil`

以 WARN 级别记录一条日志消息。同时写入 Gateway 的结构化日志系统，并附加到 DSL 执行日志中（前缀 `warn:`）。

**参数**：
- `msg`（Str）— 警告消息

**返回值**：`nil`

**示例**：
```
let cl = req.content_length
if cl != nil and cl > 524288 {
  log_warn("large request body: " + to_str(cl) + " bytes")
}
```

---

### log_error

**签名**：`log_error(msg)` → `Nil`

以 ERROR 级别记录一条日志消息。同时写入 Gateway 的结构化日志系统，并附加到 DSL 执行日志中（前缀 `error:`）。

**参数**：
- `msg`（Str）— 错误消息

**返回值**：`nil`

**示例**：
```
let sig = req.header("X-Signature")
if sig == nil {
  log_error("missing signature header, rejecting request from: " + req.client_ip)
  return deny(401, "signature required")
}
```

---

## 附录：函数速查表

| 函数 | 分类 | 返回类型 | 参数数量 |
|------|------|----------|----------|
| `req.header(name)` | 请求读取 | `Str\|Nil` | 1 |
| `req.method` | 请求读取 | `Str` | 0 |
| `req.path` | 请求读取 | `Str` | 0 |
| `req.query(name)` | 请求读取 | `Str\|Nil` | 1 |
| `req.query_string` | 请求读取 | `Str\|Nil` | 0 |
| `req.cookie(name)` | 请求读取 | `Str\|Nil` | 1 |
| `req.client_ip` | 请求读取 | `Str` | 0 |
| `req.remote_ip` | 请求读取 | `Str` | 0 |
| `req.path_param(name)` | 请求读取 | `Str\|Nil` | 1 |
| `req.header_names` | 请求读取 | `List` | 0 |
| `req.scheme` | 请求读取 | `Str` | 0 |
| `req.host` | 请求读取 | `Str\|Nil` | 0 |
| `req.uri` | 请求读取 | `Str` | 0 |
| `req.content_type` | 请求读取 | `Str\|Nil` | 0 |
| `req.has_header(name)` | 请求读取 | `Bool` | 1 |
| `req.protocol` | 请求元数据 | `Str` | 0 |
| `req.grpc_service` | 请求元数据 | `Str\|Nil` | 0 |
| `req.grpc_method` | 请求元数据 | `Str\|Nil` | 0 |
| `req.content_length` | 请求元数据 | `Int\|Nil` | 0 |
| `req.client_cert_cn` | 请求元数据 | `Str\|Nil` | 0 |
| `req.client_cert_fingerprint` | 请求元数据 | `Str\|Nil` | 0 |
| `req.set_header(name, value)` | 请求修改 | `Nil` | 2 |
| `req.append_header(name, value)` | 请求修改 | `Nil` | 2 |
| `req.remove_header(name)` | 请求修改 | `Nil` | 1 |
| `req.set_uri(uri)` | 请求修改 | `Nil` | 1 |
| `req.set_host(host)` | 请求修改 | `Nil` | 1 |
| `req.set_method(method)` | 请求修改 | `Nil` | 1 |
| `resp.status` | 响应读取 | `Int\|Nil` | 0 |
| `resp.header(name)` | 响应读取 | `Str\|Nil` | 1 |
| `resp.has_header(name)` | 响应读取 | `Bool` | 1 |
| `resp.header_names` | 响应读取 | `List` | 0 |
| `resp.content_type` | 响应读取 | `Str\|Nil` | 0 |
| `resp.set_status(code)` | 响应修改 | `Nil` | 1 |
| `resp.set_header(name, value)` | 响应修改 | `Nil` | 2 |
| `resp.append_header(name, value)` | 响应修改 | `Nil` | 2 |
| `resp.remove_header(name)` | 响应修改 | `Nil` | 1 |
| `ctx.get(key)` | 上下文 | `Str\|Nil` | 1 |
| `ctx.set(key, value)` | 上下文 | `Nil` | 2 |
| `ctx.remove(key)` | 上下文 | `Nil` | 1 |
| `len(s)` | 字符串 | `Int` | 1 |
| `substr(s, start, end)` | 字符串 | `Str` | 3 |
| `to_str(v)` | 字符串 | `Str` | 1 |
| `to_int(s)` | 字符串 | `Int\|Nil` | 1 |
| `to_upper(s)` | 字符串 | `Str` | 1 |
| `to_lower(s)` | 字符串 | `Str` | 1 |
| `trim(s)` | 字符串 | `Str` | 1 |
| `split(s, delimiter)` | 字符串 | `List` | 2 |
| `join(list, delimiter)` | 字符串 | `Str` | 2 |
| `base64_encode(s)` | 编码 | `Str` | 1 |
| `base64_decode(s)` | 编码 | `Str\|Nil` | 1 |
| `url_encode(s)` | 编码 | `Str` | 1 |
| `url_decode(s)` | 编码 | `Str` | 1 |
| `hex_encode(s)` | 编码 | `Str` | 1 |
| `hex_decode(s)` | 编码 | `Str\|Nil` | 1 |
| `sha256(s)` | 哈希 | `Str` | 1 |
| `md5(s)` | 哈希 | `Str` | 1 |
| `hmac_sha256(key, data)` | 哈希 | `Str` | 2 |
| `regex_find(s, pattern)` | 正则 | `Str\|Nil` | 2 |
| `regex_replace(s, pattern, replacement)` | 正则 | `Str` | 3 |
| `time_now()` | 时间 | `Int` | 0 |
| `time_format(timestamp, format)` | 时间 | `Str` | 2 |
| `json_get(json_str, path)` | JSON | `Str\|Int\|Bool\|Nil` | 2 |
| `http.call(method, url, body [, timeout_ms [, headers]])` | 网络 | `Map` | 2–5 |
| `ip_in_cidr(ip, cidr)` | 网络 | `Bool` | 2 |
| `uuid()` | 工具 | `Str` | 0 |
| `range(start, end)` | 工具 | `List` | 2 |
| `log(msg)` | 工具 | `Nil` | 1 |
| `log_warn(msg)` | 工具 | `Nil` | 1 |
| `log_error(msg)` | 工具 | `Nil` | 1 |

---

## 相关文档

- [DSL 插件概述](./README.md)
- [语言语法参考](./language-syntax.md)
- [沙盒安全模型](./sandbox-security.md)
- [实战示例集](./examples.md)
- [字节码指南](./bytecode.md)
