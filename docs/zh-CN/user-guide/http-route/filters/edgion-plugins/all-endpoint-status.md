# AllEndpointStatus 插件

> **🔌 Edgion 扩展**
>  
> AllEndpointStatus 是 `EdgionPlugins` CRD 提供的后端端点批量探查插件，不属于标准 Gateway API。

## 什么是 AllEndpointStatus？

AllEndpointStatus 并发查询当前路由的所有后端 endpoint，将每个 endpoint 的状态码、延迟和响应体聚合为一份 JSON 响应返回给调用方。它不将请求转发给某一个后端，而是**扇出**给全部后端，因此特别适合在滚动发布时一次性验证所有 Pod 的健康状态，或在不登录服务器的情况下快速对比各副本的响应差异。

## 快速开始

### 最简单的配置

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: health-fan-out
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
  hostnames:
    - "ops.example.com"
  rules:
    - matches:
        - path:
            type: Exact
            value: /check-all
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: all-ep-status
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: all-ep-status
  namespace: production
spec:
  requestPlugins:
    - type: AllEndpointStatus
      config: {}
```

### 默认值说明

- 默认超时 2 秒／endpoint，整体 wall-clock 超时 10 秒
- 默认最多查询 20 个 endpoint，并发 10
- 默认每个 endpoint 响应体最多读取 16 KB
- 所有超出硬上限的配置值会被**静默截断**，不报错

---

## 配置参数

### 后端 TLS 与端点身份

探测遵循各 Service 及 `backendRefs` 所指定 Service 端口的有效 `BackendTLSPolicy`。
该端口会映射到每个就绪 EndpointSlice 端点的实际服务端口，支持滚动发布期间不同的目标端口。
Service 端口映射缺失或无效时，在发起任何探测前返回 HTTP 503。

没有策略时使用 HTTP；策略有效时使用 HTTPS，应用策略中的 CA 信任、hostname（SNI 与证书验证）、
额外 SAN 约束及可选客户端证书。策略客户端证书优先于匹配 Gateway 的后端客户端证书。
插件或 LinkSys 的全局 TLS 默认值不会为这些后端探测提供凭证或信任根。

每个端点在本次调用中使用独立客户端，保证结果来自对应端点，也避免后续探测复用旧策略或旧证书的连接。
同一 Service/端口组复用 TLS 配置。排队中的探测发现其捕获的服务发现、策略或选中的 Gateway 身份已变化时会失败；
已经开始的交换使用所捕获的配置完成。客户端不会跨请求缓存。

无效策略不会回退到 HTTP。策略无效或 TLS 握手失败时，仍返回 HTTP 200 聚合结果，对应端点为失败条目
（`status: null`、`body: null`，以及简短的通用 `error`）。探测不跟随重定向、不使用代理环境变量，
现有端点数量、超时、并发及响应体大小限制继续生效。

### 基础参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `timeoutMs` | Integer | `2000` | 单个 endpoint 请求超时（毫秒）。硬上限 5000 ms |
| `wallTimeoutMs` | Integer | `10000` | 整个扇出操作的 wall-clock 超时（毫秒）。硬上限 10000 ms。超时的 endpoint 在结果中标记为 `"wall timeout exceeded"` |
| `maxEndpoints` | Integer | `20` | 最多查询的 endpoint 数量。硬上限 50 |
| `maxBodySize` | Integer | `16384` | 单个 endpoint 响应体读取上限（字节）。硬上限 16384（16 KB），超出部分流式丢弃 |
| `concurrencyLimit` | Integer | `10` | 并发请求数上限 |

### 覆盖参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `methodOverride` | String | 无 | 覆盖发往 endpoint 的 HTTP 方法，如 `"GET"` |
| `pathOverride` | String | 无 | 覆盖发往 endpoint 的请求路径，如 `"/health"` |

### 调试参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `includeResponseHeaders` | Boolean | `false` | 是否在结果中包含各 endpoint 的响应头 |

### 安全说明

所有参数均有硬上限，超出部分取三层最小值：`min(用户配置, 全局 YAML 配置上限, 硬上限)`。超出上限的值会被静默截断，不返回错误。

---

## 响应格式

插件返回 `200 OK`，`Content-Type: application/json`。响应体是一个 JSON 对象，包含 `summary` 块和 `backends` 数组；每个后端下有一个 `endpoints` 数组（每个端点一个元素）。字段名使用 snake_case。

```json
{
  "summary": {
    "total_backends": 1,
    "total_endpoints": 2,
    "success_count": 1,
    "failure_count": 1,
    "truncated": false,
    "timeout_ms": 2000,
    "wall_timeout_ms": 10000,
    "total_latency_ms": 42,
    "total_response_bytes": 17,
    "wall_timeout_hit": false
  },
  "backends": [
    {
      "name": "api-service",
      "namespace": "production",
      "port": 8080,
      "endpoint_count": 2,
      "endpoints": [
        {
          "address": "10.0.0.1:8080",
          "status": 200,
          "latency_ms": 12,
          "body": "{\"status\":\"ok\"}",
          "body_truncated": false,
          "error": null
        },
        {
          "address": "10.0.0.2:8080",
          "status": null,
          "latency_ms": 0,
          "body": null,
          "body_truncated": false,
          "error": "wall timeout exceeded"
        }
      ]
    }
  ]
}
```

### `summary` 字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `total_backends` | Integer | 探查的后端数量 |
| `total_endpoints` | Integer | 所有后端探查的端点总数 |
| `success_count` | Integer | 返回 2xx/3xx 状态码的端点数 |
| `failure_count` | Integer | 未记录 2xx/3xx 状态码的端点数：状态码在该范围之外，或因传输/时限失败没有状态码 |
| `truncated` | Boolean | 端点数超过 `maxEndpoints` 并被截断时为 `true` |
| `timeout_ms` | Integer | 实际生效的单端点超时 |
| `wall_timeout_ms` | Integer | 实际生效的 wall-clock 超时 |
| `total_latency_ms` | Integer | 整个扇出操作的 wall-clock 耗时（毫秒） |
| `total_response_bytes` | Integer | 所有端点读取的响应体总字节数 |
| `wall_timeout_hit` | Boolean | wall timeout 取消了某些端点时为 `true` |

### `backends[]` 字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `name` | String | 后端 Service 名称 |
| `namespace` | String | 后端命名空间 |
| `port` | Integer | 后端端口 |
| `endpoint_count` | Integer | 该后端的端点数量 |
| `endpoints` | Array | 各端点结果（见下方） |

### `endpoints[]` 字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `address` | String | 端点地址（`host:port`） |
| `status` | Integer | HTTP 状态码；探查在收到响应前失败时为 `null` |
| `latency_ms` | Integer | 往返延迟（毫秒） |
| `body` | String | 响应体（截断到 `maxBodySize`）；探查失败或无响应体时为 `null` |
| `body_truncated` | Boolean | 响应体在 `maxBodySize` 处被截断时为 `true` |
| `headers` | Object | 响应头。仅当 `includeResponseHeaders: true` 时存在 |
| `error` | String | 错误描述；成功时为 `null` |

当端点因连接失败或 `wallTimeoutMs` 超时而失败时，`status` 为 `null`，`error` 字段携带失败原因（连接错误，或 `"wall timeout exceeded"`）。

---

## 常见配置场景

### 1. 最简探活（仅关心状态码）

适用于滚动发布验证，不需要响应体详情。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: liveness-check
  namespace: default
spec:
  requestPlugins:
    - type: AllEndpointStatus
      config:
        timeoutMs: 1000
        methodOverride: "GET"
        pathOverride: "/healthz"
```

### 2. 自定义健康检查路径

将探活请求转向 `/health`，使用 HEAD 方法减少响应体传输。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: health-probe
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
  hostnames:
    - "ops.example.com"
  rules:
    - matches:
        - path:
            type: Exact
            value: /pod-health
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: pod-health-check
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: pod-health-check
  namespace: default
spec:
  requestPlugins:
    - type: AllEndpointStatus
      config:
        methodOverride: "HEAD"
        pathOverride: "/health"
        timeoutMs: 500
        wallTimeoutMs: 3000
        maxEndpoints: 30
```

### 3. 带响应头的详细调试

当需要对比各副本的响应头（如 `X-Pod-Name`、`X-Version`）时，启用 `includeResponseHeaders`。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: debug-pods
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
  hostnames:
    - "ops.example.com"
  rules:
    - matches:
        - path:
            type: Exact
            value: /debug/all-pods
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: debug-all-ep
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: debug-all-ep
  namespace: default
spec:
  requestPlugins:
    - type: AllEndpointStatus
      config:
        methodOverride: "GET"
        pathOverride: "/info"
        includeResponseHeaders: true
        maxBodySize: 4096
        concurrencyLimit: 5
```

---

## 常见问题

### Q1：为什么响应体被截断了？

**A**：`maxBodySize` 默认 16384 字节（16 KB），且硬上限不可超过。超出部分在流式读取时直接丢弃。如果需要完整响应体，建议在应用层缩减响应体内容，或直接访问单个 endpoint。

### Q2：配置了 `timeoutMs: 10000`，为什么实际生效的是 5000？

**A**：`timeoutMs` 的硬上限是 5000 ms，超出部分被静默截断。这是安全设计，防止慢速后端长时间阻塞扇出操作。

### Q3：结果中某些 endpoint 的 `status` 是 `null`，是什么原因？

**A**：有两种可能：
1. 连接失败（网络不通、端口未监听）
2. `wallTimeoutMs` 到期时该 endpoint 尚未响应，被强制取消（`summary.wall_timeout_hit` 此时为 `true`）

通过查看 `error` 字段可以区分两者——它携带失败原因（连接错误，或 `"wall timeout exceeded"`）。

### Q4：这个插件会影响正常流量吗？

**A**：AllEndpointStatus 是**终止型插件**——匹配该路由的请求全部由插件处理并返回，不会再转发给后端。建议配置专用路径（如 `/check-all`），而不要挂载在业务路径上。

---

## 相关文档

- [DebugAccessLogToHeader 插件](debug-access-log.md) — 调试响应头注入
- [Mock 插件](mock.md) — 静态响应返回
- [DirectEndpoint 插件](direct-endpoint.md) — 指定单个 endpoint 转发
- [RateLimit 插件](rate-limit.md) — 速率限制
