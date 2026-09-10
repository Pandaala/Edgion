# 插件组合与执行顺序

当一个请求匹配 HTTPRoute 规则时，Edgion 会在多个代理阶段依次执行来自多个来源的过滤器和插件。本页详细描述各插件的执行时机，帮助你推断执行顺序、短路行为，以及 Gateway API 过滤器与通过 `ExtensionRef` 引用的 `EdgionPlugins` 之间的交互。

## 插件来源

单次请求可从三个来源获取插件：

| 来源 | 声明方式 | 作用域 |
|------|----------|--------|
| **全局插件** | `EdgionGatewayConfig.spec.globalPluginsRef`（`EdgionPlugins` 引用列表） | 网关处理的所有请求 |
| **路由规则插件** | HTTPRoute 的 `rules[].filters`——标准 Gateway API 过滤器和 `ExtensionRef` 条目 | 匹配到的规则 |
| **BackendRef 插件** | `rules[].backendRefs[].filters` | 选中的后端 |

每个来源将插件贡献到一个或多个执行阶段。直接写在 `rules[].filters` 中的标准 Gateway API 过滤器（例如 `type: RequestHeaderModifier`），在内部会映射到与同类 `EdgionPlugins` 插件相同的插件机制，因此两个家族共享一条有序的处理流水线。

## 执行阶段

Edgion 将插件执行划分为四个阶段，与代理生命周期对应：

| 阶段 | 执行时机 | 同步 / 异步 |
|------|----------|-------------|
| **请求阶段** | 路由匹配后、联系上游之前 | 异步 |
| **上游响应（Header）阶段** | 上游响应头到达时 | 同步 |
| **上游响应体阶段** | 从上游流式传回的每个响应体 chunk | 同步 |
| **上游响应（异步）阶段** | 保留的异步响应钩子（当前无内建插件映射于此） | 异步 |

每种插件类型只在特定阶段有效。若将插件放置在其不支持的阶段，路由将以 fail-closed 方式失败（见[失败关闭行为](#失败关闭行为)）。

## 跨来源优先级

### 请求阶段

请求阶段插件按固定优先级跨三个来源依次执行：

```text
全局请求插件  →  路由规则插件  →  BackendRef 插件
```

每一层都可以在下一层执行前短路请求。例如，全局 IP 限制插件拒绝客户端后，请求将在任何路由或后端插件执行前终止。

### 响应阶段

响应 Header 阶段按以下优先级运行：

```text
路由规则响应插件  →  BackendRef 响应插件  →  全局响应插件
```

这遵循洋葱模型：最靠近上游的路由和后端插件先于包裹整个网关的全局插件运行。请求阶段插件排入队列的响应头操作（例如提前求值的 `ResponseHeaderModifier` 的 `set`）将首先被排空，响应插件因此能观察到这些变更。

响应体阶段（由 `BandwidthLimit` 等流式插件使用）先执行路由规则插件，再执行全局插件。

## 单个来源内的顺序

在同一来源和同一阶段内，插件按声明顺序执行：

- 标准 Gateway API 过滤器按其在 `rules[].filters` 中的顺序运行。
- `EdgionPlugins` 资源内的插件按 `spec.requestPlugins`、`spec.upstreamResponseFilterPlugins`、`spec.upstreamResponseBodyFilterPlugins`、`spec.upstreamResponsePlugins` 中的声明顺序运行。

## Gateway API 过滤器与阶段的映射

在 `rules[].filters` 中编写标准过滤器时，Edgion 会自动将其路由到正确的阶段：

| Filter `type` | 阶段 |
|---------------|------|
| `RequestHeaderModifier` | 请求阶段 |
| `RequestRedirect` | 请求阶段（终止） |
| `URLRewrite` | 请求阶段 |
| `RequestMirror` | 请求阶段（非阻塞） |
| `ResponseHeaderModifier` | 上游响应（Header）阶段 |
| `ExtensionRef` | 请求阶段 + 上游响应（Header）阶段 + 上游响应体阶段 |

`ExtensionRef` 比较特殊：被引用的 `EdgionPlugins` 资源可为任意阶段声明插件，因此一个 `ExtensionRef` 条目会同时挂载到请求、响应 Header 和响应体三个阶段。运行时各阶段只执行该资源中适用于本阶段的插件。

> 负载均衡策略类型的 `ExtensionRef` 不受支持。请改用 `EdgionBackendTrafficPolicy` 配置负载均衡；负载均衡策略引用不进入插件流水线。

## ExtensionRef：将路由指向 EdgionPlugins

`ExtensionRef` 是 Edgion 用于将扩展插件挂载到路由的标准 Gateway API 机制，通过名称引用一个 `EdgionPlugins` 资源：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: api-guard
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: api-guard
  namespace: production
spec:
  requestPlugins:
    - type: Cors
      config:
        allowOrigins: "https://app.example.com"
    - type: RateLimit
      config:
        rate: 100
        burst: 50
```

### 跨路由复用插件

`EdgionPlugins` 资源内部的 `type: ExtensionRef` 嵌套形式（插件引用插件）不受支持。复用插件请使用以下两种方式之一：

- **聚合模式**：将多个插件放入同一个 `EdgionPlugins` 资源，路由只需一个 `ExtensionRef`。
- **并列模式**：在 `rules[].filters` 中写多个 `ExtensionRef` 条目，各自指向不同的 `EdgionPlugins` 资源，按列出顺序执行。

```yaml
filters:
  # 先执行
  - type: ExtensionRef
    extensionRef:
      group: edgion.io
      kind: EdgionPlugins
      name: auth-plugins
  # 后执行
  - type: ExtensionRef
    extensionRef:
      group: edgion.io
      kind: EdgionPlugins
      name: rate-limit-plugins
```

## 短路行为

任何请求阶段插件都可以终止请求。当插件返回终止性结果（重定向、认证拒绝、Mock 响应或 gRPC 错误）时，运行时会立即停止请求阶段循环，设置响应，并跳过后续的请求阶段插件。`RequestRedirect` 是典型的终止性过滤器：它发送 301、302、303、307 或 308 响应，上游永远不会被访问。规则级重定向规则应省略 `backendRefs`；Kubernetes CRD 会拒绝同时包含两者的规则。

## 失败关闭行为

若路由中任意插件的配置无效，Edgion **不会**构建该插件，也**不会**静默跳过它。取而代之的是，整条路由以 fail-closed 方式失败：在请求阶段最前面插入一个门控，在上游被访问之前以 `500` 拒绝所有匹配请求。这确保配置错误的安全插件永远不会被绕过。Controller 同时会在资源状态上暴露配置错误信息。

## 相关文档

- [过滤器总览](./overview.md)
- [HTTPRoute 总览](../overview.md)
- [Gateway API 标准过滤器](./gateway-api/request-header-modifier.md)
- [Edgion 扩展插件](./edgion-plugins/README.md)
