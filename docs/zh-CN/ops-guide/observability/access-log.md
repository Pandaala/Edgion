# Access Log 用户指南

> **🔌 Edgion 扩展**
> 
> Access Log 格式和插件日志结构是 Edgion Gateway 的特有功能。

## 概述

Edgion Gateway 为每个请求生成详细的访问日志（access log），记录请求的完整生命周期信息，包括客户端信息、路由匹配、后端响应、插件执行日志等。

日志默认以 JSON 格式输出，便于日志分析工具解析。

## 日志位置

- **默认路径**: `logs/edgion_access.log`
- **格式**: JSON (每行一条日志)

## 日志结构

### 基础字段

每条 access log 包含以下主要字段：

```json
{
  "ts": 1767089146376,
  "request_info": { ... },
  "match_info": { ... },
  "backend_context": { ... },
  "plugin_logs": [ ... ],
  "errors": [ ... ]
}
```

### 1. 时间戳 (`ts`)

请求的时间戳，毫秒级 Unix 时间。

**示例**:
```json
"ts": 1767089146376
```

### 2. 请求信息 (`request_info`)

记录客户端和请求的基本信息：

| 字段 | 说明 | 示例 |
|------|------|------|
| `client-addr` | 客户端 TCP 连接地址 | `"127.0.0.1"` |
| `client-port` | 客户端端口 | `54882` |
| `remote-addr` | 实际客户端 IP（考虑代理后） | `"203.0.113.1"` |
| `x-trace-id` | 请求追踪 ID | `"eff524a7-10a4-483c-..."` |
| `host` | 请求的 Host 头 | `"test.example.com"` |
| `path` | 请求路径 | `"/health"` |
| `status` | 响应状态码 | `200` |
| `x-forwarded-for` | X-Forwarded-For 头（如有） | `"203.0.113.1, 198.51.100.2"` |
| `discover_protocol` | 自动识别的协议类型 | `"grpc"`, `"websocket"` |
| `grpc_service` | gRPC 服务名（gRPC 请求） | `"test.TestService"` |
| `grpc_method` | gRPC 方法名（gRPC 请求） | `"SayHello"` |

**示例**:
```json
"request_info": {
  "client-addr": "127.0.0.1",
  "client-port": 54882,
  "remote-addr": "127.0.0.1",
  "x-trace-id": "eff524a7-10a4-483c-a534-3c50b1199aa0",
  "host": "test.example.com",
  "path": "/health",
  "status": 200
}
```

### 3. 路由匹配信息 (`match_info`)

记录匹配到的路由规则：

| 字段 | 说明 | 示例 |
|------|------|------|
| `rns` | 路由命名空间 | `"edge"` |
| `rn` | 路由名称 | `"test-http"` |
| `rule_id` | 规则 ID（从 0 开始） | `0` |
| `match_id` | 匹配项 ID（从 0 开始） | `0` |

**示例**:
```json
"match_info": {
  "rns": "edge",
  "rn": "test-http",
  "rule_id": 0,
  "match_id": 0
}
```

如果路由未匹配，`match_info` 字段将不出现。

### 4. 后端信息 (`backend_context`)

记录后端服务和上游连接信息：

| 字段 | 说明 |
|------|------|
| `name` | 后端服务名称 |
| `namespace` | 后端服务命名空间 |
| `upstreams` | 上游连接尝试列表（支持重试） |

**上游连接信息** (`upstreams`):

| 字段 | 说明 | 单位 |
|------|------|------|
| `ip` | 上游 IP 地址 | - |
| `port` | 上游端口 | - |
| `status` | 上游响应状态码 | - |
| `ct` | 连接时间 (Connect Time) | 毫秒 |
| `ht` | 首字节响应时间 (Header Time) | 毫秒 |
| `bt` | 首包响应时间 (Body Time) | 毫秒 |
| `err` | 错误信息（如有） | - |

**示例**:
```json
"backend_context": {
  "name": "test-http",
  "namespace": "edge",
  "upstreams": [
    {
      "ip": "127.0.0.1",
      "port": 30001,
      "status": 200,
      "ct": 0,
      "ht": 0,
      "bt": 0
    }
  ]
}
```

**多次重试示例**:
```json
"backend_context": {
  "name": "test-service",
  "namespace": "edge",
  "upstreams": [
    {
      "ip": "10.0.1.5",
      "port": 8080,
      "status": 502,
      "ct": 1,
      "ht": 100,
      "err": ["Connection reset"]
    },
    {
      "ip": "10.0.1.6",
      "port": 8080,
      "status": 200,
      "ct": 2,
      "ht": 50,
      "bt": 50
    }
  ]
}
```

如果路由未匹配或请求被插件终止，`backend_context` 将为 `null`。

### 5. 插件日志 (`plugin_logs`)

记录插件执行的详细日志，按执行阶段（stage）分组：

```json
"plugin_logs": [
  {
    "stage": "request_filters",
    "logs": [
      {
        "name": "BasicAuth",
        "time_cost": 5,
        "log": ["Auth success; "]
      },
      {
        "name": "Cors",
        "time_cost": 2,
        "log": ["CORS resp set; "]
      }
    ]
  },
  {
    "stage": "upstream_response_filters",
    "logs": [
      {
        "name": "ResponseHeaderModifier",
        "time_cost": 1,
        "log": ["Header modified; "]
      }
    ]
  }
]
```

**插件日志字段说明**:

| 字段 | 说明 | 示例 |
|------|------|------|
| `stage` | 执行阶段 | `"request_filters"`, `"upstream_response_filters"` |
| `name` | 插件名称 | `"BasicAuth"`, `"Cors"` |
| `time_cost` | 执行耗时（微秒） | `5` |
| `log` | 日志内容数组 | `["Auth success; "]` |
| `log_full` | 日志缓冲区是否已满 | `true` (仅在已满时出现) |

**常见插件日志内容**:

| 插件 | 日志内容 |
|------|----------|
| **BasicAuth** | `Auth success` - 认证成功<br>`Auth failed` - 认证失败<br>`Anonymous` - 匿名访问 |
| **Cors** | `CORS resp set` - CORS 头已设置<br>`Preflight handled` - Preflight 已处理<br>`Origin rejected` - Origin 被拒绝 |
| **CSRF** | `Token verified` - Token 验证成功<br>`Token set` - Token 已设置<br>`Token mismatch` - Token 不匹配<br>`No token in header` - 缺少 token |
| **IPRestriction** | `Allowed` - IP 允许访问<br>`Denied` - IP 被拒绝 |
| **Mock** | `Mock returned` - 返回 mock 响应 |

**`log_full` 说明**:

插件日志使用固定大小的缓冲区（100 字节），当插件输出的日志超过此限制时，会截断并设置 `log_full: true`：

```json
{
  "name": "CustomPlugin",
  "time_cost": 10,
  "log": ["Entry 1; ", "Entry 2; ", "..."],
  "log_full": true
}
```

这表示该插件的日志被截断了，可能需要检查插件逻辑或优化日志输出。

### 6. 错误信息 (`errors`)

记录请求处理过程中的错误：

```json
"errors": ["RouteNotFound"]
```

**常见错误码**:

| 错误码 | 说明 |
|--------|------|
| `RouteNotFound` | 未找到匹配的路由 |
| `XffHeaderTooLong` | X-Forwarded-For 头过长 |
| `BackendNotFound` | 后端服务未找到 |
| `UpstreamConnectError` | 上游连接失败 |

## 日志示例

### 基础 HTTP 请求

```json
{
  "ts": 1767089146376,
  "request_info": {
    "client-addr": "127.0.0.1",
    "client-port": 54882,
    "remote-addr": "127.0.0.1",
    "x-trace-id": "eff524a7-10a4-483c-a534-3c50b1199aa0",
    "host": "test.example.com",
    "path": "/health",
    "status": 200
  },
  "match_info": {
    "rns": "edge",
    "rn": "test-http",
    "rule_id": 0,
    "match_id": 0
  },
  "backend_context": {
    "name": "test-http",
    "namespace": "edge",
    "upstreams": [
      {
        "ip": "127.0.0.1",
        "port": 30001,
        "status": 200,
        "ct": 0,
        "ht": 0,
        "bt": 0
      }
    ]
  }
}
```

### 带插件的请求

```json
{
  "ts": 1767089146377,
  "request_info": {
    "client-addr": "127.0.0.1",
    "client-port": 54883,
    "remote-addr": "127.0.0.1",
    "x-trace-id": "d82dcc21-2412-4582-8726-3e7a7becc58b",
    "host": "api.example.com",
    "path": "/api/data",
    "status": 200
  },
  "match_info": {
    "rns": "edge",
    "rn": "api-route",
    "rule_id": 0,
    "match_id": 0
  },
  "backend_context": {
    "name": "api-backend",
    "namespace": "edge",
    "upstreams": [
      {
        "ip": "10.0.1.5",
        "port": 8080,
        "status": 200,
        "ct": 2,
        "ht": 15,
        "bt": 15
      }
    ]
  },
  "plugin_logs": [
    {
      "stage": "request_filters",
      "logs": [
        {
          "name": "BasicAuth",
          "time_cost": 8,
          "log": ["Auth success; "]
        },
        {
          "name": "Cors",
          "time_cost": 2,
          "log": ["CORS resp set; "]
        }
      ]
    }
  ]
}
```

### 路由未匹配

```json
{
  "ts": 1767089149015,
  "request_info": {
    "client-addr": "127.0.0.1",
    "client-port": 54892,
    "remote-addr": "127.0.0.1",
    "x-trace-id": "c8f3e4ec-e1b6-47f0-8941-be21bb86ee0f",
    "host": "wrong-hostname.example.com",
    "path": "/test",
    "status": 404
  },
  "errors": ["RouteNotFound"],
  "backend_context": null
}
```

### gRPC 请求

```json
{
  "ts": 1767089148470,
  "request_info": {
    "client-addr": "127.0.0.1",
    "client-port": 54885,
    "remote-addr": "127.0.0.1",
    "x-trace-id": "358c2727-250a-405a-92b2-7b5017cf95d0",
    "host": "grpc.example.com",
    "path": "/test.TestService/SayHello",
    "status": 200,
    "discover_protocol": "grpc",
    "grpc_service": "test.TestService",
    "grpc_method": "SayHello"
  },
  "backend_context": {
    "name": "test-grpc",
    "namespace": "edge",
    "upstreams": [
      {
        "ip": "127.0.0.1",
        "port": 30021,
        "status": 200,
        "ct": 0,
        "ht": 1,
        "bt": 1
      }
    ]
  }
}
```

## 日志分析建议

### 1. 使用 jq 分析日志

```bash
# 查看所有 4xx 错误
cat logs/edgion_access.log | jq 'select(.request_info.status >= 400 and .request_info.status < 500)'

# 统计最慢的请求（根据 ht）
cat logs/edgion_access.log | jq -r '[.request_info.path, .backend_context.upstreams[0].ht] | @tsv' | sort -k2 -n

# 查看插件执行耗时
cat logs/edgion_access.log | jq '.plugin_logs[] | .logs[] | select(.time_cost > 100)'

# 查看所有认证失败的请求
cat logs/edgion_access.log | jq 'select(.plugin_logs[]?.logs[]? | select(.name == "BasicAuth" and (.log[] | contains("Auth failed"))))'
```

### 2. 关注关键指标

- **`ht` (Header Time)**: 后端响应首字节时间，高值可能表示后端性能问题
- **`ct` (Connect Time)**: 连接建立时间，高值可能表示网络问题
- **`plugin_logs.time_cost`**: 插件执行耗时，高值可能表示插件性能问题
- **`log_full: true`**: 插件日志被截断，可能需要优化插件

### 3. 错误排查

1. **路由未匹配** (`RouteNotFound`): 检查 `host` 和 `path` 是否与路由配置匹配
2. **认证失败**: 查看 `plugin_logs` 中的 `BasicAuth` 日志
3. **CORS 错误**: 查看 `plugin_logs` 中的 `Cors` 日志，检查是否有 `Origin rejected`
4. **后端错误**: 查看 `backend_context.upstreams[].status` 和 `err` 字段

## 最佳实践

1. **设置 X-Trace-ID**: 在客户端请求中设置 `X-Trace-ID` 头，便于追踪请求链路
2. **定期轮转日志**: 使用 `logrotate` 或类似工具定期轮转 `edgion_access.log`
3. **集成日志系统**: 将 `edgion_access.log` 集成到 ELK、Loki 等日志分析平台
4. **监控关键指标**: 根据 `edgion_access.log` 设置告警，如高延迟、高错误率等

## Related Features

- [Gateway 概述](../gateway/overview.md)
- [Preflight 策略](../gateway/preflight-policy.md)
