# DSL 插件

> **🔌 Edgion 扩展**
> 
> DSL 是 `EdgionPlugins` CRD 提供的自定义脚本插件，不属于标准 Gateway API。

## 概述

DSL 插件允许通过内联 EdgionDSL 脚本自定义请求处理逻辑。脚本在安全的沙箱 VM 中执行，支持读取请求信息、设置 header、拒绝请求等操作。适用于需要灵活定制但不值得开发独立插件的场景。

**特性**：
- 沙箱执行，资源限制可配置
- 支持源码和预编译字节码两种加载方式
- 可配置的错误策略

## 快速开始

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: dsl-plugin
spec:
  requestPlugins:
    - enable: true
      type: Dsl
      config:
        name: "header-check"
        source: |
          let token = req.header("X-Api-Token")
          if token == nil {
            return deny(403, "missing X-Api-Token header")
          }
```

---

## 配置参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `name` | String | ✅ | 无 | 脚本名称（用于日志和调试） |
| `source` | String | ✅* | 无 | DSL 源代码 |
| `bytecode` | String | ✅* | 无 | 预编译的字节码（Base64） |
| `maxSteps` | Integer | ❌ | `10000` | 最大执行步数 |
| `maxLoopIterations` | Integer | ❌ | `100` | 最大循环迭代次数 |
| `maxCallCount` | Integer | ❌ | `500` | 最大函数调用次数 |
| `maxStackDepth` | Integer | ❌ | `128` | 最大栈深度 |
| `maxStringLen` | Integer | ❌ | `8192` | 最大字符串长度 |
| `errorPolicy` | String | ❌ | `Ignore` | 错误策略：`Ignore` / `Deny` / `DenyWith` |

\* `source` 和 `bytecode` 二选一，至少提供一个。

### 错误策略

| 策略 | 行为 |
|------|------|
| `Ignore` | 忽略脚本执行错误，继续处理请求 |
| `Deny` | 脚本出错时拒绝请求（返回 500） |
| `DenyWith` | 脚本出错时返回自定义状态码和消息 |

---

## 常见配置场景

### 场景 1：Header 检查

```yaml
requestPlugins:
  - enable: true
    type: Dsl
    config:
      name: "require-api-token"
      source: |
        let token = req.header("X-Api-Token")
        if token == nil {
          return deny(403, "missing X-Api-Token header")
        }
      errorPolicy: deny
```

### 场景 2：条件路由标记

```yaml
requestPlugins:
  - enable: true
    type: Dsl
    config:
      name: "set-routing-tag"
      source: |
        let ua = req.header("User-Agent")
        if ua != nil && contains(ua, "Mobile") {
          req.set_header("X-Client-Type", "mobile")
        } else {
          req.set_header("X-Client-Type", "desktop")
        }
```

### 场景 3：严格资源限制

```yaml
requestPlugins:
  - enable: true
    type: Dsl
    config:
      name: "strict-check"
      source: |
        let method = req.method()
        if method == "DELETE" {
          return deny(405, "DELETE not allowed")
        }
      maxSteps: 1000
      maxLoopIterations: 10
      errorPolicy: ignore
```

---

## DSL API 参考

以下列出所有可用 API，标记 **NEW** 的为新增 API。

### 请求读取 (req.*)

| API | 参数 | 返回值 | 说明 |
|-----|------|--------|------|
| `req.header(name)` | 1 string | string \| nil | 获取请求头 |
| `req.method()` | 0 | string | HTTP 方法 |
| `req.path()` | 0 | string | 请求路径 |
| `req.query(name)` | 1 string | string \| nil | 获取查询参数 |
| `req.query_string()` | 0 | string | 完整查询字符串 |
| `req.cookie(name)` | 1 string | string \| nil | 获取 Cookie 值 |
| `req.client_ip()` | 0 | string | 客户端 IP |
| `req.remote_ip()` | 0 | string | 远端 IP（直连地址） |
| `req.path_param(name)` | 1 string | string \| nil | URL 路径参数 |
| `req.header_names()` | 0 | list | 所有请求头名称（最多 256 个） |
| `req.scheme()` | 0 | string | 请求协议（http/https） |
| `req.host()` | 0 | string | Host 头值 |
| `req.uri()` | 0 | string | 完整 URI |
| `req.content_type()` | 0 | string \| nil | Content-Type 头 |
| `req.has_header(name)` | 1 string | bool | 检查请求头是否存在 |

### 请求元数据 — **NEW**

| API | 参数 | 返回值 | 说明 |
|-----|------|--------|------|
| `req.protocol` | 0 | string | 协议类型："http"、"grpc"、"grpc-web"、"websocket" **NEW** |
| `req.grpc_service` | 0 | string \| nil | gRPC 服务名（从路径解析） **NEW** |
| `req.grpc_method` | 0 | string \| nil | gRPC 方法名（从路径解析） **NEW** |
| `req.content_length` | 0 | int \| nil | 解析后的 Content-Length 值 **NEW** |
| `req.client_cert_cn` | 0 | string \| nil | mTLS 客户端证书 Common Name **NEW** |
| `req.client_cert_fingerprint` | 0 | string \| nil | 客户端证书指纹哈希 **NEW** |

### 请求修改 (req.*)

| API | 参数 | 返回值 | 说明 |
|-----|------|--------|------|
| `req.set_header(name, value)` | 2 strings | nil | 设置请求头 |
| `req.append_header(name, value)` | 2 strings | nil | 追加请求头 |
| `req.remove_header(name)` | 1 string | nil | 删除请求头 |
| `req.set_uri(uri)` | 1 string | nil | 重写请求 URI |
| `req.set_host(host)` | 1 string | nil | 重写 Host 头 |
| `req.set_method(method)` | 1 string | nil | 修改 HTTP 方法 |

### 响应读取 (resp.*)

| API | 参数 | 返回值 | 说明 |
|-----|------|--------|------|
| `resp.status` | 0 | int | 响应状态码 |
| `resp.header(name)` | 1 string | string \| nil | 获取响应头 |
| `resp.has_header(name)` | 1 string | bool | 检查响应头是否存在 |
| `resp.header_names()` | 0 | list | 所有响应头名称（最多 256 个） **NEW** |
| `resp.content_type` | 0 | string \| nil | 响应 Content-Type **NEW** |

### 响应修改 (resp.*)

| API | 参数 | 返回值 | 说明 |
|-----|------|--------|------|
| `resp.set_header(name, value)` | 2 strings | nil | 设置响应头 |
| `resp.append_header(name, value)` | 2 strings | nil | 追加响应头 |
| `resp.remove_header(name)` | 1 string | nil | 删除响应头 |
| `resp.set_status(code)` | 1 int (100-599) | nil | 设置响应状态码 **NEW** |

### 上下文存储 (ctx.*)

| API | 参数 | 返回值 | 说明 |
|-----|------|--------|------|
| `ctx.get(key)` | 1 string | string \| nil | 获取共享上下文值 |
| `ctx.set(key, value)` | 2 strings | nil | 设置共享上下文值 |
| `ctx.remove(key)` | 1 string | nil | 删除上下文键 |

### 工具函数

| 函数 | 参数 | 返回值 | 说明 |
|------|------|--------|------|
| `contains(haystack, needle)` | 2 strings | bool | 字符串包含检查 |
| `starts_with(str, prefix)` | 2 strings | bool | 字符串前缀检查 |
| `ends_with(str, suffix)` | 2 strings | bool | 字符串后缀检查 |
| `len(str)` | 1 string/list | int | 字符串/列表长度 |
| `to_lower(str)` | 1 string | string | 转小写 |
| `to_upper(str)` | 1 string | string | 转大写 |
| `substr(str, start, end)` | 3 | string | 子字符串 |
| `to_string(val)` | 1 any | string | 转换为字符串 |
| `to_int(str)` | 1 string | int \| nil | 解析整数 |
| `base64_encode(str)` | 1 string | string | Base64 编码 |
| `base64_decode(str)` | 1 string | string | Base64 解码 |
| `url_encode(str)` | 1 string | string | URL 编码 |
| `url_decode(str)` | 1 string | string | URL 解码 |
| `sha256(str)` | 1 string | string | SHA-256 十六进制哈希 |
| `md5(str)` | 1 string | string | MD5 十六进制哈希 |
| `time_now()` | 0 | int | 当前 Unix 时间戳 |
| `regex_find(str, pattern)` | 2 strings | string \| nil | 正则首次匹配 |
| `regex_replace(str, pat, rep)` | 3 strings | string | 正则全部替换 |
| `trim(str)` | 1 string | string | 去除首尾空白 |
| `split(str, delimiter)` | 2 strings | list | 按分隔符拆分为列表 |
| `range(start, end)` | 2 ints | list | 整数范围 |
| `hmac_sha256(key, data)` | 2 strings | string | HMAC-SHA256 十六进制 **NEW** |
| `json_get(json_str, path)` | 2 strings | string \| nil | JSON 点路径提取 **NEW** |
| `uuid()` | 0 | string | 生成 UUID v4 **NEW** |
| `hex_encode(str)` | 1 string | string | 十六进制编码 **NEW** |
| `hex_decode(hex)` | 1 string | string | 十六进制解码 **NEW** |
| `time_format(timestamp, fmt)` | int + string | string | 格式化 Unix 时间戳 **NEW** |
| `ip_in_cidr(ip, cidr)` | 2 strings | bool | 检查 IP 是否在 CIDR 范围内 **NEW** |
| `join(list, delimiter)` | list + string | string | 连接列表元素 **NEW** |
| `log_warn(msg)` | 1 string | nil | 输出 WARN 日志 **NEW** |
| `log_error(msg)` | 1 string | nil | 输出 ERROR 日志 **NEW** |

### 返回语句

| 语句 | 说明 |
|------|------|
| `return deny(status, message)` | 以 HTTP 状态码和消息体拒绝请求 |
| `return allow()` | 显式允许（无操作） |
| `return next()` | 跳到下一个插件 |
| `return terminate()` | 立即终止 TCP 连接（不发送响应） **NEW** |
| `return grpc_error(code, message)` | 通过 HTTP/2 trailers 返回原生 gRPC 错误 **NEW** |

---

## gRPC 错误返回

`grpc_error()` 返回语句提供原生 gRPC 错误处理，通过 HTTP/2 trailers 发送标准 gRPC 状态码。

### 语法

```dsl
return grpc_error(code, message)
```

- **code**：gRPC 状态码（整数 0-16）。超出范围的值会被截断为 13（INTERNAL）。
- **message**：人类可读的错误消息字符串。

### 示例：gRPC 认证检查

```yaml
requestPlugins:
  - enable: true
    type: Dsl
    config:
      name: "grpc-auth"
      source: |
        if req.protocol != "grpc" {
          return next()
        }
        let token = req.header("authorization")
        if token == nil {
          return grpc_error(16, "missing authorization token")
        }
      errorPolicy: deny
```

### 阶段限制

> **警告**：`grpc_error()` **仅在 `requestPlugins` 中支持**。

| 插件阶段 | grpc_error() 支持 | 行为 |
|---------|-------------------|------|
| `requestPlugins` | 完全支持 | 发送 HTTP 200 + grpc-status trailers |
| `upstreamResponseFilterPlugins` | 不支持 | 响应已提交，降级为 terminate |
| `upstreamResponsePlugins` | 不支持 | 响应已发送，降级为 terminate |

**原因**：gRPC 错误通过 HTTP/2 trailers 传递。只有在创建新响应时（请求阶段）才能设置 trailers。一旦响应头已提交，无法再追加 trailers。

### gRPC 状态码

| 状态码 | 名称 | 说明 |
|--------|------|------|
| 0 | OK | 无错误 |
| 1 | CANCELLED | 操作已取消 |
| 2 | UNKNOWN | 未知错误 |
| 3 | INVALID_ARGUMENT | 无效参数 |
| 4 | DEADLINE_EXCEEDED | 超时 |
| 5 | NOT_FOUND | 资源未找到 |
| 6 | ALREADY_EXISTS | 资源已存在 |
| 7 | PERMISSION_DENIED | 权限被拒绝 |
| 8 | RESOURCE_EXHAUSTED | 资源耗尽 |
| 9 | FAILED_PRECONDITION | 前置条件失败 |
| 10 | ABORTED | 操作已中止 |
| 11 | OUT_OF_RANGE | 超出范围 |
| 12 | UNIMPLEMENTED | 未实现 |
| 13 | INTERNAL | 内部错误 |
| 14 | UNAVAILABLE | 服务不可用 |
| 15 | DATA_LOSS | 数据丢失 |
| 16 | UNAUTHENTICATED | 未认证 |

---

## 限制说明

| 函数 | 限制 | 说明 |
|------|------|------|
| `json_get(json, path)` | 路径最多 32 段；输入/输出最大 8192 字节 | 超出返回错误 |
| `time_format(ts, fmt)` | 格式字符串最大 256 字符 | 超出返回错误 |
| `regex_find` / `regex_replace` | 正则模式最大 1024 字节 | 超出返回错误 |
| `req.header_names()` / `resp.header_names()` | 最多返回 256 个头名称 | 超出部分截断 |
| `resp.set_status(code)` | 状态码范围 100-599 | 超出范围返回错误 |
| 所有字符串返回值 | 受 `maxStringLen` 限制（默认 8192） | 返回 StringTooLong 错误 |

---

## 注意事项

1. DSL 脚本在沙箱中执行，无法访问文件系统或网络
2. 超出资源限制（`maxSteps`、`maxLoopIterations` 等）的脚本会被终止
3. 使用 `bytecode` 可以跳过运行时编译，提高性能
4. `errorPolicy` 建议在生产环境中设置为 `Ignore` 或 `DenyWith`，避免脚本错误导致服务中断

---

## 完整示例

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: dsl-route
spec:
  parentRefs:
    - name: my-gateway
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
            name: dsl-validation
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: dsl-validation
spec:
  requestPlugins:
    - enable: true
      type: Dsl
      config:
        name: "request-validation"
        source: |
          let token = req.header("Authorization")
          if token == nil {
            return deny(401, "Authorization header required")
          }
          let method = req.method()
          if method == "DELETE" {
            let admin = req.header("X-Admin-Token")
            if admin == nil {
              return deny(403, "Admin token required for DELETE")
            }
          }
        errorPolicy: deny
```

## 相关文档

- [过滤器总览](../overview.md)
- [插件组合与引用](../plugin-composition.md)
