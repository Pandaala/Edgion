# Mock 插件

> **🔌 Edgion 扩展**
> 
> Mock 是 `EdgionPlugins` CRD 提供的 Mock 响应插件，不属于标准 Gateway API。

## 概述

Mock 插件返回预设的 HTTP 响应，不转发请求到上游服务。适用于 API 原型开发、接口测试、健康检查端点、错误模拟等场景。

## 快速开始

### 最简配置

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: mock-plugin
  namespace: production
spec:
  requestPlugins:
    - type: Mock
      config:
        statusCode: 200
        body: '{"status": "ok", "message": "Service is healthy"}'
        contentType: "application/json"
```

### 默认值说明

| 字段 | 默认值 | 含义 |
|------|--------|------|
| `statusCode` | `200` | 返回的 HTTP 状态码 |
| `contentType` | `"application/json"` | 响应的 `Content-Type` |
| `terminate` | `true` | 返回 Mock 响应后停止后续处理 |

---

## 配置参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `statusCode` | Integer | `200` | 最终 HTTP 状态码。有效范围：`200`–`599` |
| `body` | String | 无 | 响应体内容（JSON、XML、纯文本等） |
| `headers` | Object | 无 | 自定义响应头，以键值对形式表示；名称和值必须符合 HTTP 响应头语法 |
| `contentType` | String | `"application/json"` | 合法的 `Content-Type` 响应头值 |
| `delay` | Integer | 无 | 响应前的延迟（毫秒）。最大值：`30000` |
| `terminate` | Boolean | `true` | 是否在运行后终止请求处理 |

### `terminate` 行为

- `true`（默认）：直接返回 Mock 响应，不转发到上游，也不执行后续插件。
- `false`：将配置的响应头和 `Content-Type` 加入待写入队列，然后继续执行后续插件和上游转发。此模式下，`statusCode` 和 `body` 不会生成本地响应。

### 响应头约束

- 响应头名称必须是合法的 HTTP token，且最长为 256 字节。响应头值（包括 `contentType`）必须是合法的 HTTP 响应头值。
- 响应头名称按大小写不敏感比较时必须唯一。
- 不得在 `headers` 中配置 `Content-Type`、`Content-Length` 或 `Transfer-Encoding`。请使用 `contentType` 设置 `Content-Type`；响应分帧头由 Gateway 管理。
- 配置延迟时，插件会在延迟结束后才将响应头加入待写入队列。

---

## 常见配置场景

### 1. 健康检查端点

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: mock-health
  namespace: production
spec:
  requestPlugins:
    - type: Mock
      config:
        statusCode: 200
        body: '{"status": "healthy"}'
```

### 2. 带自定义响应头的 API 原型

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: mock-prototype
  namespace: production
spec:
  requestPlugins:
    - type: Mock
      config:
        statusCode: 200
        body: |
          {
            "users": [
              {"id": 1, "name": "Alice"},
              {"id": 2, "name": "Bob"}
            ]
          }
        headers:
          X-Mock: "true"
          Cache-Control: "no-cache"
```

### 3. 模拟错误响应

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: mock-error
  namespace: staging
spec:
  requestPlugins:
    - type: Mock
      config:
        statusCode: 503
        body: '{"error": "Service temporarily unavailable"}'
        contentType: "application/json"
```

### 4. 带延迟的响应（模拟慢接口）

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: mock-slow
  namespace: staging
spec:
  requestPlugins:
    - type: Mock
      config:
        statusCode: 200
        body: '{"result": "slow response"}'
        # 延迟上限为 30000ms
        delay: 2000
```

### 5. 非终止模式（设置响应头后继续处理）

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: mock-tag
  namespace: production
spec:
  requestPlugins:
    - type: Mock
      config:
        statusCode: 200
        terminate: false
        headers:
          X-Mock-Flag: "true"
```

---

## 常见问题

### Q1：为什么 Mock 之后的其他插件不执行？

**A**：当 `terminate: true`（默认值）时，Mock 立即返回并跳过所有后续插件和上游转发。若需要请求继续处理，请设置 `terminate: false`。

### Q2：能只对路由上的某些请求返回 Mock 响应吗？

**A**：可以。用单独的 HTTPRoute 规则（路径 / 请求头 / 方法匹配）匹配目标请求，仅对该规则附加 Mock 插件。插件本身运行时始终响应。

### Q3：`delay` 超过 30 秒的值似乎被忽略了？

**A**：`delay` 的上限为 30000ms（30 秒）。插件验证会拒绝超出该值的配置。

---

## 完整配置示例

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: mock-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - "mock.example.com"
  rules:
    - matches:
        - path:
            type: Exact
            value: /health
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: health-mock
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: health-mock
  namespace: production
spec:
  requestPlugins:
    - type: Mock
      config:
        statusCode: 200
        body: '{"status": "ok", "version": "1.0.0"}'
        contentType: "application/json"
        headers:
          X-Health-Check: "true"
```

## 相关链接

- [过滤器目录](../README.md)
- [CORS 插件](./cors.md)
