# EdgionDSL 插件接口扩展计划

> 基于对 DSL 引擎全栈（Parser → AST → Compiler → Bytecode → VM → Builtins）
> 及 PluginSession trait 的深度分析，梳理当前缺失的通用接口并规划实现路径。
> Date: 2026-03-30

---

## 一、现有接口盘点

### 1.1 `req.*` 读取（15 个）

| 接口 | 参数 | 返回 | 说明 |
|------|------|------|------|
| `req.header(name)` | 1 | `str\|nil` | 获取请求头 |
| `req.has_header(name)` | 1 | `bool` | 判断请求头是否存在 |
| `req.header_names()` | 0 | `list` | 所有请求头名 |
| `req.method` | 0 | `str` | HTTP 方法 |
| `req.path` | 0 | `str` | 请求路径（不含 query） |
| `req.uri` | 0 | `str` | 完整 URI（path + query） |
| `req.host` | 0 | `str\|nil` | Host 头 |
| `req.scheme` | 0 | `str` | 协议 scheme（http/https） |
| `req.query(name)` | 1 | `str\|nil` | 指定 query 参数 |
| `req.query_string` | 0 | `str\|nil` | 完整 query string |
| `req.cookie(name)` | 1 | `str\|nil` | 指定 cookie |
| `req.client_ip` | 0 | `str` | TCP 直连 IP |
| `req.remote_ip` | 0 | `str` | 真实客户端 IP（经 proxy header 提取） |
| `req.path_param(name)` | 1 | `str\|nil` | 路由路径参数 |
| `req.content_type` | 0 | `str\|nil` | Content-Type 头 |

### 1.2 `req.*` 修改（6 个）

| 接口 | 参数 | 说明 |
|------|------|------|
| `req.set_header(name, value)` | 2 | 设置请求头 |
| `req.append_header(name, value)` | 2 | 追加请求头 |
| `req.remove_header(name)` | 1 | 删除请求头 |
| `req.set_uri(uri)` | 1 | 改写上游 URI |
| `req.set_host(host)` | 1 | 改写上游 Host |
| `req.set_method(method)` | 1 | 改写上游 Method |

### 1.3 `resp.*` 读写（6 个）

| 接口 | 参数 | 说明 |
|------|------|------|
| `resp.status` | 0 | 读取响应状态码（`int\|nil`） |
| `resp.header(name)` | 1 | 读取响应头 |
| `resp.has_header(name)` | 1 | 判断响应头是否存在 |
| `resp.set_header(name, value)` | 2 | 设置响应头 |
| `resp.append_header(name, value)` | 2 | 追加响应头 |
| `resp.remove_header(name)` | 1 | 删除响应头 |

### 1.4 `ctx.*` 上下文（3 个）

| 接口 | 参数 | 说明 |
|------|------|------|
| `ctx.get(key)` | 1 | 获取上下文变量 |
| `ctx.set(key, value)` | 2 | 设置上下文变量 |
| `ctx.remove(key)` | 1 | 删除上下文变量 |

### 1.5 工具函数（18 个）

| 函数 | 参数 | 说明 |
|------|------|------|
| `log(msg)` | 1 | 插件日志 |
| `len(v)` | 1 | 字符串/列表长度 |
| `substr(s, start, end)` | 3 | 子串（UTF-8 安全） |
| `to_str(v)` / `to_int(v)` | 1 | 类型转换 |
| `to_upper(s)` / `to_lower(s)` | 1 | 大小写转换 |
| `base64_encode(s)` / `base64_decode(s)` | 1 | Base64 编解码 |
| `url_encode(s)` / `url_decode(s)` | 1 | URL 编解码 |
| `sha256(s)` / `md5(s)` | 1 | 摘要哈希 |
| `time_now()` | 0 | 当前 UNIX 时间戳（秒） |
| `regex_find(s, pattern)` | 2 | 正则查找 |
| `regex_replace(s, pattern, repl)` | 3 | 正则替换 |
| `trim(s)` | 1 | 去首尾空白 |
| `split(s, delim)` | 2 | 字符串分割为列表 |
| `range(start, end)` | 2 | 生成整数列表 |

### 1.6 流程控制

| 语句 | 映射到 PluginRunningResult | 说明 |
|------|----------------------------|------|
| `return next()` | `GoodNext` | 放行，继续执行下一个插件 |
| `return deny(status, body)` | `ErrResponse { status, body }` | 拒绝，返回指定状态码和响应体 |

---

## 二、PluginSession 已有但 DSL 未暴露的能力

| PluginSession 方法 | 说明 | 是否适合暴露到 DSL |
|---|---|---|
| `set_response_status(u16)` | 修改响应状态码 | **适合** — upstream_response_filter 阶段常用 |
| `set_remote_addr(addr)` | 设置真实客户端 IP | 适合 — 但与 RealIp 插件功能重叠 |
| `client_cert_info()` | 获取 mTLS 客户端证书信息 | **适合** — 只读，安全 |
| `set_direct_endpoint(info)` | 直连指定端点 | 谨慎 — 需安全控制 |
| `set_internal_jump(info)` | 内部跳转到指定 BackendRef | 谨慎 — 需安全控制 |
| `set_external_jump(info)` | 外部域名跳转 | 谨慎 — 需安全控制 |
| `set_mirror_state(state)` | 设置请求镜像 | 低优先级 |
| `write_response_header/body` | 直接写响应 | 不适合 — DSL 是同步 VM，此为 async |
| `shutdown()` | 关闭会话 | 不适合直接暴露 — 通过 `return terminate()` 间接实现 |

---

## 三、新增接口 TODO

### P0：流水线控制

- [ ] **`return terminate()`** — 静默终止请求
  - 映射到已有的 `PluginRunningResult::ErrTerminateRequest`
  - 场景：恶意请求静默丢弃、防爬虫黑洞策略、gRPC 场景静默断开
  - 改动：`StmtKind::ReturnTerminate` + `OpCode::ReturnTerminate` + parser 识别 `return terminate()`
  - 复杂度：**低**
  - 文件：`ast.rs`, `parser.rs`, `compiler.rs`, `bytecode.rs`, `vm.rs`

```
// 示例
if req.header("X-Attack-Sig") != nil {
    log("attack detected, terminating")
    return terminate()
}
```

- [ ] **`resp.set_status(code)`** — 修改响应状态码
  - 底层：`PluginSession::set_response_status()`
  - 场景：将上游 5xx 改写为自定义码、gRPC 状态码映射、A/B 测试
  - 改动：`BuiltinId::RespSetStatus` + builtins.rs + compiler 映射
  - 复杂度：**低**

```
// 示例
if resp.status == 502 {
    resp.set_status(503)
    resp.set_header("Retry-After", "30")
}
```

### P1：请求元数据

- [ ] **`req.protocol`** — 获取协议类型
  - 值域：`"http"`, `"https"`, `"grpc"`, `"grpc-web"`
  - 底层：`ctx.request_info.discover_protocol`
  - 需要 PluginSession 新增方法或通过 ctx 暴露
  - 场景：编写 HTTP/gRPC 通用脚本
  - 复杂度：**低**

```
if req.protocol == "grpc" {
    req.set_header("X-GRPC-Processed", "true")
}
```

- [ ] **`req.grpc_service`** / **`req.grpc_method`** — gRPC 服务和方法名
  - 底层：`ctx.request_info.grpc_service` / `ctx.request_info.grpc_method`
  - 场景：gRPC 服务级访问控制
  - 复杂度：**中**（需 PluginSession 扩展）

```
if req.protocol == "grpc" && req.grpc_service == "admin.AdminService" {
    return deny(403, "admin service blocked")
}
```

- [ ] **`req.content_length`** — 请求体大小（整数）
  - 底层：解析 `Content-Length` header 为 i64
  - 场景：请求体大小限制
  - 复杂度：**低**

```
if req.content_length > 10485760 {
    return deny(413, "Request Entity Too Large")
}
```

- [ ] **`req.tls_version`** — TLS 版本（`str|nil`）
  - 值域：`"TLSv1.2"`, `"TLSv1.3"` 或 `nil`（非 TLS）
  - 场景：强制 HTTPS、TLS 版本最低要求
  - 复杂度：**中**（需从 Pingora session 提取 TLS info）

```
if req.tls_version == nil {
    return deny(403, "HTTPS required")
}
```

- [ ] **`req.client_cert_cn`** / **`req.client_cert_fingerprint`** — mTLS 证书信息
  - 底层：`PluginSession::client_cert_info()` 已有
  - 场景：mTLS 客户端身份验证
  - 复杂度：**低**

```
let cn = req.client_cert_cn
if cn != "allowed-service" {
    return deny(403, "Invalid client certificate")
}
```

### P2：gRPC 专用流水线控制

- [ ] **`return grpc_error(code, message)`** — gRPC 原生错误返回
  - gRPC 错误在 HTTP/2 层面是 200 响应 + `grpc-status` / `grpc-message` trailers
  - 当前 `return deny(403, ...)` 对 gRPC 客户端不友好（返回 HTTP 错误而非 gRPC status）
  - 需要新的 `PluginRunningResult` 变体或 Session 方法
  - 可选：内置 gRPC status 常量 `GRPC_OK=0`, `GRPC_CANCELLED=1`, ..., `GRPC_PERMISSION_DENIED=7` 等
  - 复杂度：**高**

```
if req.protocol == "grpc" {
    return grpc_error(7, "PERMISSION_DENIED: not allowed")
}
```

- [ ] **`return deny()` gRPC 自动适配**（备选方案）
  - 当 `req.protocol == "grpc"` 时，`return deny(status, body)` 自动将 HTTP status 映射为 gRPC status code
  - 映射规则：403 → 7(PERMISSION_DENIED), 404 → 5(NOT_FOUND), 429 → 8(RESOURCE_EXHAUSTED) 等
  - 对用户透明，不需要学习新语法
  - 复杂度：**中**

### P2：响应操作扩展

- [ ] **`resp.header_names()`** — 响应头名列表
  - 与 `req.header_names()` 对称
  - 场景：批量清理内部响应头
  - 需要 PluginSession 扩展（当前没有 `response_headers()` 方法）
  - 复杂度：**低**

```
for name in resp.header_names() {
    if name.starts_with("X-Internal-") {
        resp.remove_header(name)
    }
}
```

- [ ] **`resp.content_type`** — 语法糖
  - 等价于 `resp.header("Content-Type")`
  - 复杂度：**极低**（纯 compiler 映射）

### P2：路由与后端控制

- [ ] **`req.set_upstream(host, port)`** — 动态上游路由
  - 底层：`PluginSession::set_direct_endpoint()`
  - 场景：灰度发布、故障切换、A/B 测试
  - **安全考量**：需在 DslConfig 中增加 `allow_routing: bool` 开关（默认 false）
  - 复杂度：**中**

```
if req.header("X-Canary") == "true" {
    req.set_upstream("canary-svc.default.svc", 8080)
}
```

- [ ] **`req.set_backend(name)`** — 动态选择 BackendRef
  - 底层：`PluginSession::set_internal_jump()`
  - 场景：根据请求特征动态选择后端
  - 同样需要安全开关
  - 复杂度：**中**

### P3：工具函数扩展

- [ ] **`hmac_sha256(key, data)`** — HMAC-SHA256 签名
  - 场景：请求签名验证、webhook 验证
  - 复杂度：**低**（用 `hmac` crate）

```
let expected = hmac_sha256(ctx.get("secret"), req.path + req.query_string)
if req.header("X-Signature") != expected {
    return deny(401, "Invalid signature")
}
```

- [ ] **`json_get(json_str, path)`** — 轻量 JSON 路径提取
  - 底层：用 `serde_json` 实现简单 dot-notation 提取
  - 场景：从 header/ctx 中提取 JSON 字段
  - 复杂度：**中**

```
let meta = json_get(req.header("X-Meta"), "user.role")
if meta == "admin" {
    ctx.set("is_admin", "true")
}
```

- [ ] **`uuid()`** — 生成 UUID v4
  - 场景：请求 ID 注入、trace ID 生成
  - 复杂度：**低**（用 `uuid` crate）

```
if req.header("X-Request-ID") == nil {
    req.set_header("X-Request-ID", uuid())
}
```

- [ ] **`hex_encode(s)`** / **`hex_decode(s)`** — 十六进制编解码
  - 底层：已有 `hex` crate 依赖（sha256 实现中使用）
  - 复杂度：**极低**

- [ ] **`time_format(timestamp, format)`** — 时间格式化
  - 底层：`chrono` 或手写 ISO 8601 格式化
  - 复杂度：**低**

```
resp.set_header("X-Response-Time", time_format(time_now(), "%Y-%m-%dT%H:%M:%SZ"))
```

- [ ] **`ip_in_cidr(ip, cidr)`** — IP CIDR 匹配
  - 场景：DSL 内联 IP 段判断（比 IpRestriction 插件更灵活）
  - 复杂度：**低**（用 `ipnet` 或手写）

```
if ip_in_cidr(req.client_ip, "10.0.0.0/8") {
    ctx.set("network", "internal")
}
```

- [ ] **`join(list, delim)`** — 列表拼接为字符串
  - 与 `split()` 对称
  - 复杂度：**极低**

### P3：可观测性增强

- [ ] **`log_warn(msg)`** / **`log_error(msg)`** — 分级日志
  - 当前只有 `log()` 一个级别
  - 底层：用 `tracing::warn!` / `tracing::error!` 替代 `tracing::debug!`
  - 复杂度：**低**

```
if resp.status >= 500 {
    log_error("upstream error: " + to_str(resp.status))
}
```

- [ ] **`metric_inc(name)`** / **`metric_inc_by(name, value)`** — 自定义 Prometheus 指标
  - 场景：DSL 脚本内上报业务指标
  - 需要与 metrics 子系统集成（全局 Counter 注册表）
  - **安全考量**：需限制指标名数量防止基数爆炸
  - 复杂度：**中**

```
if req.path.starts_with("/api/v2") {
    metric_inc("dsl_api_v2_requests")
}
```

---

## 四、HTTP vs gRPC 关闭流水线差异

### 4.1 当前状态

| DSL 语句 | PluginRunningResult | HTTP 行为 | gRPC 行为 |
|---|---|---|---|
| `return next()` | `GoodNext` | 继续流水线 | 继续流水线 |
| `return deny(s, b)` | `ErrResponse { status, body }` | 返回 HTTP 错误响应 | 返回 HTTP 错误（gRPC 客户端看到非 200 响应） |
| *(不存在)* | `ErrTerminateRequest` | — | — |

### 4.2 问题

1. **`return deny()` 对 gRPC 不友好**：gRPC 客户端期望收到 HTTP 200 + `grpc-status` trailer，而非 HTTP 403 等。当前 `deny` 返回的是裸 HTTP 错误码。
2. **缺少静默终止**：无法利用 `ErrTerminateRequest` 直接断开连接。
3. **缺少协议感知**：DSL 脚本无法知道当前请求是 HTTP 还是 gRPC，无法分别处理。

### 4.3 建议实现方案

| 关闭方式 | HTTP 行为 | gRPC 行为 | DSL 语法 | 优先级 |
|---|---|---|---|---|
| 静默终止 | 关闭 TCP 连接 | RST_STREAM | `return terminate()` | P0 |
| HTTP 错误 | HTTP status + body | HTTP status + body（现有） | `return deny(s, b)` | 已有 |
| gRPC 原生错误 | N/A | HTTP 200 + grpc-status trailer | `return grpc_error(code, msg)` | P2 |
| deny 自动适配 | 不变 | 自动映射 HTTP→gRPC status | `return deny(s, b)` + 自动检测 | P2 备选 |

### 4.4 gRPC Status Code 映射参考

| HTTP Status | gRPC Status Code | gRPC Status Name |
|---|---|---|
| 400 | 3 | INVALID_ARGUMENT |
| 401 | 16 | UNAUTHENTICATED |
| 403 | 7 | PERMISSION_DENIED |
| 404 | 5 | NOT_FOUND |
| 408 | 4 | DEADLINE_EXCEEDED |
| 409 | 6 | ALREADY_EXISTS |
| 429 | 8 | RESOURCE_EXHAUSTED |
| 500 | 13 | INTERNAL |
| 501 | 12 | UNIMPLEMENTED |
| 502 | 14 | UNAVAILABLE |
| 503 | 14 | UNAVAILABLE |
| 504 | 4 | DEADLINE_EXCEEDED |

---

## 五、实现指南

### 5.1 新增一个 DSL Builtin 的标准流程（4 步）

1. **`bytecode.rs`** — `BuiltinId` 枚举新增变体
2. **`compiler.rs`** — `resolve_namespace_method()` 或 `compile_fn_call()` 新增映射 + `builtin_expected_argc()` 补充
3. **`builtins.rs`** — `call_builtin()` 中新增 match 分支，调用 `PluginSession` 方法
4. *(可选)* **`session.rs`** — 如果需要新的 Session 能力，扩展 `PluginSession` trait + `PingoraSessionAdapter` 实现

### 5.2 新增一个流程控制语句的标准流程（5 步）

1. **`ast.rs`** — `StmtKind` 新增变体
2. **`parser.rs`** — `parse_return_stmt()` 中识别新关键词
3. **`compiler.rs`** — `compile_stmt()` 中处理新 `StmtKind` 变体
4. **`bytecode.rs`** — `OpCode` 新增变体
5. **`vm.rs`** — `execute()` 中处理新 `OpCode` + 返回对应 `PluginRunningResult`

### 5.3 安全注意事项

- 路由控制类接口（`set_upstream`, `set_backend`）需在 `DslConfig` 中增加权限开关
- 自定义指标需限制指标名数量，防止 Prometheus 基数爆炸
- `json_get` 需限制 JSON 解析深度和大小
- 所有新 builtin 都受现有 VM 安全沙箱约束（step/call budget, string length limit 等）

---

## 六、工作量估算

| 优先级 | 接口数 | 预估工作量 | 依赖 |
|---|---|---|---|
| P0 | 2 (`terminate`, `resp.set_status`) | 0.5 天 | 无外部依赖 |
| P1 | 5 (`protocol`, `grpc_*`, `content_length`, `tls_version`, `client_cert_*`) | 1-2 天 | 需扩展 PluginSession |
| P2 | 5 (`grpc_error`, `resp.header_names`, `resp.content_type`, `set_upstream`, `set_backend`) | 2-3 天 | 需扩展 PluginSession + 安全控制 |
| P3 | 9 (`hmac_sha256`, `json_get`, `uuid`, `hex_*`, `time_format`, `ip_in_cidr`, `join`, `log_*`, `metric_*`) | 2-3 天 | 部分需新 crate 依赖 |
| **总计** | **21 个新接口** | **~6-8 天** | |
