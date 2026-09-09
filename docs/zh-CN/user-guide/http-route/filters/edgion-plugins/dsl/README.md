# DSL 插件

> **🔌 Edgion 扩展**
>
> DSL 是 `EdgionPlugins` CRD 提供的自定义脚本插件，不属于标准 Gateway API。

## 概述

DSL 插件允许在不开发独立插件的情况下，通过内联 EdgionDSL 脚本灵活定制请求和响应的处理逻辑。脚本运行于内置沙箱 VM 中，每次请求独立执行，支持读写请求头、拒绝请求、修改响应状态码，以及返回原生 gRPC 错误等操作。

DSL 适合以下场景：权限前置检查、动态 Header 注入、协议级别路由标记、基于 mTLS 证书的访问控制等。对于更复杂的逻辑，建议开发专用插件。

## 特性一览

- **沙箱隔离执行**：脚本无法访问文件系统或环境变量，运行环境完全受控
- **出站 HTTP 调用**：脚本可通过 `http.call()` 调用外部 HTTP/HTTPS 服务，也可原始拉取流式发送不透明的 `req.body`，支持可配置的请求/响应体上限、超时，以及可选的 SSRF 防护（`httpBlockPrivateIps`）
- **字节码预编译**：支持 `source`（配置处理时编译）和 `bytecode`（预编译 Base64）两种加载方式，字节码模式可跳过编译开销
- **资源限制可配置**：可独立限制执行步数、循环次数、调用深度、字符串长度
- **请求 body 存储透明**：通过有界操作检查或修改不透明的 `req.body`，磁盘溢写无需先合并为一个完整字符串
- **错误策略可选**：`Ignore` / `Deny` / `DenyWith` 三种策略，适应不同容错需求
- **原生 gRPC 支持**：通过 HTTP/2 trailers 返回标准 gRPC 状态码（`grpc_error()`）
- **双阶段执行**：请求阶段（转发前）和响应阶段（上游返回后）各自独立运行

## 快速开始

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: dsl-plugin
spec:
  requestPlugins:
    - type: Dsl
      config:
        name: "header-check"
        source: |
          let token = req.header("X-Api-Token")
          if token == nil {
            return deny(403, "missing X-Api-Token header")
          }
```

## 配置参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `name` | String | ✅ | 无 | 脚本名称（用于日志和调试），长度为 1–36 个 UTF-8 字节 |
| `source` | String | ✅* | 无 | DSL 源代码 |
| `bytecode` | String | ✅* | 无 | 预编译的字节码（Base64） |
| `maxSteps` | Integer | ❌ | `10000` | 最大执行步数 |
| `maxLoopIterations` | Integer | ❌ | `100` | 最大循环迭代次数 |
| `maxCallCount` | Integer | ❌ | `500` | 最大函数调用次数 |
| `maxStackDepth` | Integer | ❌ | `128` | 最大栈深度 |
| `maxStringLen` | Integer | ❌ | `8192` | 最大字符串长度 |
| `maxBodyScanPasses` | Integer | ❌ | `4` | 请求 body 累计扫描预算，以物理 body 上限的完整扫描次数计，范围 `1`–`16` |
| `errorPolicy` | String | ❌ | `Ignore` | 错误策略：`Ignore` / `Deny` / `DenyWith` |
| `httpMaxTimeoutMs` | Integer | ❌ | `5000` | `http.call()` 单次调用超时上限（毫秒） |
| `httpMaxRequestBodyBytes` | Integer | ❌ | `1048576` | `http.call()` 发送不透明 `Body` 的最大字节数（`1`–`33554432`，同时受网关物理请求 body 上限约束） |
| `httpMaxResponseBodyBytes` | Integer | ❌ | `1048576` | `http.call()` 响应体最大字节数 |
| `httpBlockPrivateIps` | Boolean | ❌ | `false` | 拦截 `http.call()` 对私有/回环/链路本地 IP 的调用 |
| `linkSysRefs` | Vec<{namespace, name}> | ❌† | `[]` | LinkSys Redis 资源列表。每个 `name` 是作为 `redis.*` 调用第一个参数的别名 |

\* `source` 和 `bytecode` 二选一，至少提供一个。
† 每个 Redis 别名都应在 `linkSysRefs` 中声明。静态检查会拒绝能够确定的未知别名；
动态或有歧义的表达式可能在运行时返回 `ok=false`。
`rateLimitPolicies[].link` 始终在加载时校验。

使用 `rateLimitPolicies`（数组，默认 `[]`，最多 64 条）为
[`redis.rate_limit`](api-reference.md#redis-rate_limit) 声明原子限流策略。
每条策略包含 `name`、`link`、`namespace`、`rate`、`interval`，以及可选的
`algorithm`（默认 `SlidingWindow`）和 `timeoutMs`（默认 `500`）。

## 错误策略

| 策略 | 行为 |
|------|------|
| `Ignore` | 忽略脚本执行错误，继续处理请求 |
| `Deny` | 脚本出错时拒绝请求（返回 500） |
| `DenyWith` | 脚本出错时返回自定义状态码和消息 |

无论 `errorPolicy` 如何配置，请求 body 失败都固定 fail-close。VM 执行前的 body
gate 在请求体超过上限时返回空响应体的 413，捕获超时时返回空响应体的 408，拒绝捕获
或其他捕获失败时返回空响应体的 500。VM 开始执行后，Body 可用性、存储 I/O、扫描
预算和修改发布失败返回 HTTP 500 和通用响应体
`Request body processing failed`。这是内部硬失败分类，不是新增的可配置失败策略。

## 执行阶段

DSL 插件在两个独立阶段运行，分别对应 `EdgionPlugins` 的不同字段：

### `requestPlugins` — 请求阶段

在请求转发至上游之前执行。此阶段可以：

- 读取和修改请求头、URI、Host、HTTP 方法
- 读取查询参数、Cookie、路径参数、客户端 IP
- 读取 mTLS 客户端证书信息（`req.client_cert_cn`、`req.client_cert_fingerprint`）
- 通过有界文本、字节范围、哈希和确定 JSONPath 操作检查不透明的请求 `Body`
- 通过 `clear`、`append`、`prepend`、`remove` 和字面量 `replace` 事务式修改请求 body
- 通过 `deny()` 拒绝请求并返回 HTTP 错误
- 通过 `grpc_error()` 返回原生 gRPC 状态码（仅此阶段支持）
- 通过 `ctx.set()` 向后续插件传递上下文值

### `upstreamResponseFilterPlugins` — 响应阶段

在收到上游响应后、将响应发送至客户端之前执行。此阶段可以：

- 读取上游响应状态码（`resp.status`）和响应头
- 修改响应头（`resp.set_header()`、`resp.append_header()`、`resp.remove_header()`）
- 修改响应状态码（`resp.set_status()`）
- 读取请求阶段设置的上下文值（`ctx.get()`）
- 读取请求阶段 `RequestBodyBuffer` 已缓存的 `req.body`

响应阶段的 DSL 绝不会启动请求 body 捕获。请求 body 修改若不在
`requestPlugins` 中，会在配置校验阶段被拒绝。

> **注意**：`grpc_error()` 仅在请求阶段有效。在响应阶段调用会降级为 `terminate()`，因为响应头已提交，无法再追加 HTTP/2 trailers。

## 文档导航

| 文档 | 内容 |
|------|------|
| [语言语法参考](./language-syntax.md) | 类型系统、变量、运算符、控制流 |
| [API 参考手册](./api-reference.md) | 所有内置函数的详细文档 |
| [沙盒安全模型](./sandbox-security.md) | 安全机制、资源限制、错误处理 |
| [实战示例集](./examples.md) | 真实场景示例与最佳实践 |
| [字节码指南](./bytecode.md) | 预编译字节码的使用方法 |

## 相关文档

- [过滤器总览](../../overview.md)
- [插件组合与引用](../../plugin-composition.md)
