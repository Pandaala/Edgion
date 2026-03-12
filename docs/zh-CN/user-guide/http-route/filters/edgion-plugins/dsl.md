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
