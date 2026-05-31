# Access Log

> **🔌 Edgion 扩展**
>
> access log 格式和插件日志结构是 Edgion Gateway 的特有功能，不属于标准 Gateway API 规范。

Edgion Gateway 为每个请求生成一条结构化 JSON access log，记录完整的请求生命周期：客户端信息、路由匹配、后端响应、每个插件的执行日志和错误。这是网关诊断单个请求的主要机制。

## 概述

access log 是回答*特定请求*相关问题的正确工具——"带 `x-trace-id=abc` 的请求发生了什么？"。聚合指标（见[监控指标](./metrics.md)）回答集群级问题；access log 回答单请求级问题。

每条日志是一行上的单个 JSON 对象，文件可直接被 `jq`、Loki、Elasticsearch 或任何面向行的日志管道消费。

```
client request ──► Gateway ──► backend
                     │
                     └──► one JSON line ──► logs/edgion_access.log
```

## 配置

access log 在网关配置的 `access_log` 段下配置。

### 最小配置

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
```

此配置使用默认队列和轮转行为启用日志。`path` 除非为绝对路径，否则相对于 `work_dir` 解析。

### 生产配置

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
      # 反压作用于请求路径之前的内存队列
      queue_size: 80000
      rotation:
        strategy: daily
        max_files: 7
```

### 配置参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `enabled` | Boolean | `false` | 启用 access log 通道 |
| `output.localFile.path` | String | none | 日志文件路径（除非为绝对路径，否则相对于 `work_dir`）。使用 `localFile` 时**必填** |
| `output.localFile.queue_size` | Integer | none | 反压生效前的内存队列大小 |
| `output.localFile.rotation.strategy` | Enum | none | 轮转策略。示例值：`daily` |
| `output.localFile.rotation.max_files` | Integer | none | 保留的轮转文件最大数量 |
| `output.elasticsearch` | Object | none | 将 access log 发送到 Elasticsearch（通过 `link_sys`），可替代或补充本地文件 |

原始客户端入口 URI（`request_uri`）由单独的顶层 `access_log_options` 段控制，而非 `access_log` 块——见[特殊字段](#特殊字段)。

> `output` 块也接受 `elasticsearch` sink。当 Elasticsearch 生产者→批量循环通道被填满时，`edgion_link_sys_es_bulk_channel_full_total` 计数器递增——将其作为反压信号监控。见[监控指标](./metrics.md)。

## 日志格式

日志始终是 JSON，每行一条。可选字段为空时完全省略（而非输出 `null`），保持每行紧凑。顶层结构如下：

```json
{
  "ts": 1767089146376,
  "client-addr": "192.168.1.20",
  "host": "api.example.com",
  "path": "/health",
  "status": 200,
  "match_info": { },
  "backend_context": { },
  "plugin_logs": [ ],
  "info": [ ],
  "errors": [ ]
}
```

请求作用域字段（`client-addr`、`host`、`path`、`status` 等）直接平铺在顶层对象中；路由、后端和插件信息为嵌套结构。

## 字段参考

### 顶层字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `ts` | Integer | 条目时间戳，纪元毫秒 |
| `match_info` | Object | 匹配的路由标识（无路由匹配时省略） |
| `backend_context` | Object | 后端标识和上游时序（无后端时省略/`null`） |
| `plugin_logs` | Array\<Object\> | 按阶段分组的每插件执行日志（为空时省略） |
| `info` | Array\<String\> | 网关内部信息事件（为空时省略） |
| `errors` | Array\<String\> | 处理过程中捕获的错误原因（为空时省略） |

### 请求字段

直接平铺在顶层对象中。每个字段不存在时省略。

| 字段 | 类型 | 说明 |
|------|------|------|
| `client-addr` | String | 客户端 TCP 连接地址 |
| `client-port` | Integer | 客户端 TCP 端口 |
| `remote-addr` | String | 代理解析后的真实客户端 IP（如来自 `X-Forwarded-For`） |
| `x-trace-id` | String | 每请求追踪 ID；客户端传递相同值可关联日志 |
| `host` | String | 请求 `Host` 头 |
| `path` | String | 规范化后的请求路径 |
| `status` | Integer | 响应状态码 |
| `x-forwarded-for` | String | `X-Forwarded-For` 头值（存在时） |
| `discover_protocol` | String | 自动检测的协议。示例：`grpc`、`websocket` |
| `grpc_service` | String | gRPC 服务名（仅 gRPC 请求） |
| `grpc_method` | String | gRPC 方法名（仅 gRPC 请求） |
| `sni` | String | TLS 服务器名指示（仅 TLS 连接） |
| `tls_id` | String | TLS 连接/会话标识符（仅 TLS 连接） |
| `request_uri` | String | 客户端入口原始 URI；**仅当** `access_log_options.include_raw_uri: true` 时输出 |

### `match_info` 字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `rns` | String | 路由命名空间 |
| `rn` | String | 路由名称 |
| `rule_id` | Integer | 匹配的规则索引（从 0 开始） |
| `match_id` | Integer | 匹配的匹配项索引（从 0 开始） |
| `sv` | Integer | Controller→Gateway 同步版本；为 0 时省略 |

无路由匹配时，整个 `match_info` 对象省略。

### `backend_context` 字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `name` | String | 后端服务名称 |
| `namespace` | String | 后端服务命名空间 |
| `upstreams` | Array\<Object\> | 每次上游尝试的条目（重试时产生多条） |

#### `upstreams[]` 子字段

| 字段 | 类型 | 单位 | 说明 |
|------|------|------|------|
| `ip` | String | — | 上游 IP 地址 |
| `port` | Integer | — | 上游端口 |
| `status` | Integer | — | 上游响应状态码 |
| `ct` | Integer | ms | 连接时间 |
| `ht` | Integer | ms | 首字节时间（到响应第一字节） |
| `bt` | Integer | ms | 响应体首字节时间 |
| `et` | Integer | ms | 本次尝试的总耗时 |
| `err` | Array\<String\> | — | 本次尝试的错误信息（为空时省略） |

无后端使用时（路由未匹配，或请求被插件短路），`backend_context` 为 `null` 或省略。

### `plugin_logs` 字段

`plugin_logs` 是阶段组的数组；每个组包含一个执行阶段的每插件日志。

```json
"plugin_logs": [
  {
    "stage": "request_filters",
    "logs": [
      { "name": "BasicAuth", "time_cost": 5, "log": ["Auth success"] },
      { "name": "Cors", "time_cost": 2, "log": ["CORS resp set"] }
    ]
  }
]
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `stage` | String | 执行阶段。示例：`request_filters`、`upstream_response_filters` |
| `logs[].name` | String | 插件名称 |
| `logs[].time_cost` | Integer | 插件执行时间（**微秒**） |
| `logs[].log` | Array\<String\> | 插件输出的日志行 |
| `logs[].log_full` | Boolean | 仅在插件日志缓冲区溢出并丢弃条目时出现且为 `true` |

常见插件日志行：

| 插件 | 日志行 | 含义 |
|------|--------|------|
| BasicAuth | `Auth success` / `Auth failed` / `Anonymous` | 认证结果 |
| Cors | `CORS resp set` / `Preflight handled` / `Origin rejected` | CORS 处理结果 |
| CSRF | `Token verified` / `Token set` / `Token mismatch` / `No token in header` | CSRF 处理结果 |
| IPRestriction | `Allowed` / `Denied` | IP 过滤结果 |
| Mock | `Mock returned` | 返回了 mock 响应 |

`log_full: true` 表示插件输出的日志数据超过了每插件缓冲区的容量，后续行被丢弃。将其视为需要精简插件日志输出的提示，而非错误。

### `errors` 取值

| 值 | 说明 |
|----|------|
| `RouteNotFound` | 请求无匹配路由 |
| `XffHeaderTooLong` | `X-Forwarded-For` 头超过允许长度 |
| `BackendNotFound` | 后端服务无法解析 |
| `UpstreamConnectError` | 上游连接失败 |

## 特殊字段

### 原始请求 URI

默认情况下日志记录规范化后的 `path`。要同时捕获客户端发送的原始 URI（规范化/重写之前），在顶层 `access_log_options` 段启用 `include_raw_uri`：

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log

access_log_options:
  include_raw_uri: true
```

原始 URI 随后出现在 `request_uri` 字段中。调试路径重写或 URL 编码问题时有用，但会增加每行大小，因此仅在排查时启用。

## 常见配置场景

### 1. 快速本地调试

使用默认配置将日志写入本地文件——开发期间查看网关行为的最简方式：

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
```

### 2. 生产环境按日轮转

每天保留一个文件，保留一周历史：

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
      queue_size: 80000
      rotation:
        strategy: daily
        max_files: 7
```

### 3. 排查路径重写问题

调试重写问题时同时捕获规范化路径和客户端原始 URI：

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log

access_log_options:
  include_raw_uri: true
```

## 日志示例

### 基础 HTTP 请求

```json
{
  "ts": 1767089146376,
  "client-addr": "192.168.1.20",
  "client-port": 54882,
  "remote-addr": "192.168.1.20",
  "x-trace-id": "eff524a7-10a4-483c-a534-3c50b1199aa0",
  "host": "test.example.com",
  "path": "/health",
  "status": 200,
  "match_info": { "rns": "production", "rn": "api-route", "rule_id": 0, "match_id": 0 },
  "backend_context": {
    "name": "api-service",
    "namespace": "production",
    "upstreams": [
      { "ip": "10.0.0.5", "port": 8080, "status": 200, "ct": 0, "ht": 0, "bt": 0, "et": 1 }
    ]
  }
}
```

### 带重试的请求

```json
{
  "ts": 1767089146377,
  "host": "api.example.com",
  "path": "/api/data",
  "status": 200,
  "backend_context": {
    "name": "api-service",
    "namespace": "production",
    "upstreams": [
      { "ip": "10.0.0.5", "port": 8080, "status": 502, "ct": 1, "ht": 100, "et": 100, "err": ["Connection reset"] },
      { "ip": "10.0.0.6", "port": 8080, "status": 200, "ct": 2, "ht": 50, "bt": 50, "et": 52 }
    ]
  }
}
```

### 路由未匹配

```json
{
  "ts": 1767089149015,
  "host": "wrong-hostname.example.com",
  "path": "/test",
  "status": 404,
  "errors": ["RouteNotFound"],
  "backend_context": null
}
```

### gRPC 请求

```json
{
  "ts": 1767089148470,
  "host": "grpc.example.com",
  "path": "/test.TestService/SayHello",
  "status": 200,
  "discover_protocol": "grpc",
  "grpc_service": "test.TestService",
  "grpc_method": "SayHello",
  "backend_context": {
    "name": "grpc-service",
    "namespace": "production",
    "upstreams": [
      { "ip": "10.0.0.21", "port": 8080, "status": 200, "ct": 0, "ht": 1, "bt": 1, "et": 1 }
    ]
  }
}
```

## 使用 jq 分析日志

```bash
# 所有 4xx 响应
cat logs/edgion_access.log | jq 'select(.status >= 400 and .status < 500)'

# 按上游首字节时间排序最慢的请求
cat logs/edgion_access.log | jq -r '[.path, .backend_context.upstreams[0].ht] | @tsv' | sort -k2 -n

# 耗时超过 100 微秒的插件执行
cat logs/edgion_access.log | jq '.plugin_logs[]?.logs[]? | select(.time_cost > 100)'

# 端到端追踪一个请求
cat logs/edgion_access.log | jq 'select(.["x-trace-id"] == "eff524a7-10a4-483c-a534-3c50b1199aa0")'
```

重点关注的字段：

- `ht`（首字节时间）：后端首字节时间；值高说明后端延迟高。
- `ct`（连接时间）：值高说明网络或连接池问题。
- `et`（总耗时）：每次尝试的总时间，含重试。
- `plugin_logs[].time_cost`：插件开销（微秒）。
- `log_full: true`：插件日志缓冲区溢出。

## 常见问题

### Q1：我启用了 access log，但文件为空。为什么？

**A**：确认 `enabled: true` 已设置，且 `output.localFile.path` 解析到可写位置。相对路径相对于 `work_dir` 解析，请检查网关进程的实际工作目录。同时确认请求确实到达了网关。

### Q2：为什么某些字段从日志行中缺失？

**A**：可选字段为空时省略，而非写入 `null`。例如，无路由匹配时 `match_info` 不存在，`grpc_service` 只出现在 gRPC 请求，`sni` / `tls_id` 只出现在 TLS 连接。这是预期行为，保持日志行紧凑。

### Q3：如何关联多跳请求的日志？

**A**：从客户端发送 `X-Trace-ID` 头。网关将其记录为 `x-trace-id`，可以用单个 `jq` 选择器过滤一个逻辑请求的所有条目。

### Q4：请求被插件拒绝了——看哪里？

**A**：查看 `plugin_logs`。拒绝插件的 `log` 数组通常包含原因（例如 BasicAuth 的 `Auth failed` 或 Cors 的 `Origin rejected`）。当插件短路请求时，`backend_context` 通常为 `null`。

### Q5：如何获取某个路由的更详细插件诊断？

**A**：使用 debug-access-log 插件提升每插件的详细程度。见 [debug-access-log](../../user-guide/http-route/filters/edgion-plugins/debug-access-log.md)。

## 获取帮助

如遇问题：

1. 检查本文档的常见问题部分
2. 检查网关日志
3. 在 GitHub 提交 issue

## 相关文档

- [监控指标](./metrics.md)
- [debug-access-log 插件](../../user-guide/http-route/filters/edgion-plugins/debug-access-log.md)
- [edgion-cli](../edgion-cli.md)
