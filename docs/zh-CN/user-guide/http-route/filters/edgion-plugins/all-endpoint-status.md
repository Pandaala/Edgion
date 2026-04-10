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
  namespace: default
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

所有参数均有硬上限，超出部分取三层最小值：`min(用户配置, 全局 TOML 上限, 硬上限)`。超出上限的值会被静默截断，不返回错误。

---

## 响应格式

插件返回 `200 OK`，`Content-Type: application/json`，响应体为 JSON 数组，每个元素对应一个 endpoint：

```json
[
  {
    "endpoint": "10.0.0.1:8080",
    "status": 200,
    "latencyMs": 12,
    "body": "{\"status\":\"ok\"}",
    "headers": {}
  },
  {
    "endpoint": "10.0.0.2:8080",
    "status": 503,
    "latencyMs": 2001,
    "body": "service unavailable",
    "headers": {}
  }
]
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `endpoint` | String | endpoint 地址（`host:port`） |
| `status` | Integer | HTTP 状态码；连接失败时为 `-1` |
| `latencyMs` | Integer | 请求延迟（毫秒） |
| `body` | String | 响应体（截断到 `maxBodySize`） |
| `headers` | Object | 响应头（仅当 `includeResponseHeaders: true` 时存在） |

当 endpoint 因 wall timeout 被取消时，`status` 为 `-1`，`body` 为 `"wall timeout exceeded"`。

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

### Q3：结果中某些 endpoint 的 `status` 是 `-1`，是什么原因？

**A**：有两种可能：
1. 连接失败（网络不通、端口未监听）
2. `wallTimeoutMs` 到期时该 endpoint 尚未响应，被强制取消

通过查看 `body` 字段可以区分：连接失败通常有错误描述，wall timeout 时 `body` 为 `"wall timeout exceeded"`。

### Q4：这个插件会影响正常流量吗？

**A**：AllEndpointStatus 是**终止型插件**——匹配该路由的请求全部由插件处理并返回，不会再转发给后端。建议配置专用路径（如 `/check-all`），而不要挂载在业务路径上。

---

## 相关文档

- [DebugAccessLogToHeader 插件](debug-access-log.md) — 调试响应头注入
- [Mock 插件](mock.md) — 静态响应返回
- [DirectEndpoint 插件](direct-endpoint.md) — 指定单个 endpoint 转发
- [RateLimit 插件](rate-limit.md) — 速率限制
