# Mock 插件

> **🔌 Edgion 扩展**
> 
> Mock 是 `EdgionPlugins` CRD 提供的 Mock 响应插件，不属于标准 Gateway API。

## 概述

Mock 插件返回预设的 HTTP 响应，不转发请求到上游服务。适用于 API 原型开发、接口测试、健康检查端点、错误模拟等场景。

## 快速开始

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: mock-plugin
spec:
  requestPlugins:
    - enable: true
      type: Mock
      config:
        statusCode: 200
        body: '{"status": "ok", "message": "Service is healthy"}'
        contentType: "application/json"
```

---

## 配置参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `statusCode` | Integer | ❌ | `200` | HTTP 响应状态码 |
| `body` | String | ❌ | 无 | 响应体内容 |
| `headers` | Object | ❌ | 无 | 自定义响应头（key-value 对） |
| `contentType` | String | ❌ | `"application/json"` | Content-Type 类型 |
| `delay` | Integer | ❌ | 无 | 延迟响应毫秒数 |
| `terminate` | Boolean | ❌ | `true` | 是否终止请求处理 |

### terminate 行为

- `true`（默认）：直接返回 Mock 响应，不转发到上游
- `false`：设置响应状态和 header，但继续处理后续插件和上游转发

---

## 常见配置场景

### 场景 1：健康检查端点

```yaml
requestPlugins:
  - enable: true
    type: Mock
    config:
      statusCode: 200
      body: '{"status": "healthy"}'
```

### 场景 2：API 原型

```yaml
requestPlugins:
  - enable: true
    type: Mock
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

### 场景 3：模拟错误响应

```yaml
requestPlugins:
  - enable: true
    type: Mock
    config:
      statusCode: 503
      body: '{"error": "Service temporarily unavailable"}'
      contentType: "application/json"
```

### 场景 4：带延迟的响应（模拟慢接口）

```yaml
requestPlugins:
  - enable: true
    type: Mock
    config:
      statusCode: 200
      body: '{"result": "slow response"}'
      delay: 2000
```

### 场景 5：非终止模式（与其他插件配合）

```yaml
requestPlugins:
  - enable: true
    type: Mock
    config:
      statusCode: 200
      terminate: false
      headers:
        X-Mock-Flag: "true"
```

---

## 注意事项

1. `terminate: true` 时，后续的插件和上游转发都不会执行
2. Mock 插件可以与 Plugin Condition 配合使用，实现条件性 Mock
3. `delay` 会阻塞当前请求的处理线程，在高并发场景下慎用

---

## 完整示例

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: mock-api
spec:
  parentRefs:
    - name: my-gateway
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
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: health-mock
spec:
  requestPlugins:
    - enable: true
      type: Mock
      config:
        statusCode: 200
        body: '{"status": "ok", "version": "1.0.0"}'
        contentType: "application/json"
        headers:
          X-Health-Check: "true"
```

## 相关文档

- [过滤器总览](../overview.md)
- [插件组合与引用](../plugin-composition.md)
