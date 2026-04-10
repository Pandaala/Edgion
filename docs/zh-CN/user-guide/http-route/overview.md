# HTTPRoute 总览

HTTPRoute 是 Gateway API 中用于定义 HTTP 路由规则的核心资源。

## 基本结构

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: example-route
  namespace: default
spec:
  parentRefs:           # 绑定到哪个 Gateway
    - name: my-gateway
      sectionName: http
  hostnames:            # 匹配的域名
    - "example.com"
  rules:                # 路由规则列表
    - matches:          # 匹配条件
        - path:
            type: PathPrefix
            value: /api
      filters:          # 过滤器（可选）
        - type: RequestHeaderModifier
          requestHeaderModifier:
            add:
              - name: X-Custom-Header
                value: "value"
      backendRefs:      # 后端服务
        - name: api-service
          port: 8080
          weight: 100
```

关键字段说明：

| 字段 | 说明 |
|------|------|
| `parentRefs` | 将路由绑定到一个或多个 Gateway 监听器 |
| `hostnames` | 匹配请求的 Host 头（精确或通配符） |
| `rules` | 路由规则列表，每条包含 matches、filters、backendRefs |

## 核心概念

### ParentRefs

指定此路由绑定到哪个 Gateway 的哪个监听器：

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | String | 是 | Gateway 名称 |
| `namespace` | String | 否 | Gateway 命名空间（默认同路由） |
| `sectionName` | String | 否 | 监听器名称 |

一个 HTTPRoute 可以绑定多个 parentRefs，实现跨 Gateway 共享路由规则。

### Rules

每条 Rule 是一个独立的路由决策单元，包含匹配条件、过滤器和后端引用。多条 Rule 按声明顺序依次求值，第一条匹配的 Rule 生效。

### Matches

定义请求的匹配条件，支持多种维度：

- **路径匹配**：`PathPrefix`、`Exact`、`RegularExpression`
- **头部匹配**：精确匹配或正则匹配 HTTP 头
- **查询参数匹配**：匹配 URL 查询参数
- **方法匹配**：匹配 HTTP 方法（GET、POST 等）

同一 Match 内的条件为 AND 关系，多个 Match 之间为 OR 关系。详见 [匹配规则](./matches/README.md)。

### Filters / Plugins

在请求到达后端之前或响应返回客户端之前执行处理逻辑：

- **Gateway API 标准 Filter**：`RequestHeaderModifier`、`ResponseHeaderModifier`、`RequestRedirect`、`URLRewrite`、`RequestMirror` 等
- **Edgion 扩展 Plugin**：通过 `ExtensionRef` 引用 `EdgionPlugins` CRD，支持限流、认证、CORS 等高级功能

详见 [过滤器](./filters/README.md)。

### BackendRefs

定义流量转发的后端服务列表：

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | String | 是 | Service 名称 |
| `port` | Integer | 是 | 端口号 |
| `weight` | Integer | 否 | 权重（默认 `1`），用于加权轮询 |

支持多后端加权分流，实现灰度发布和蓝绿部署。详见 [后端配置](./backends/README.md)。

## 完整示例

以下示例展示了一个包含多条规则的 HTTPRoute，覆盖路径匹配、头部修改、流量分流等常用功能：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: app-route
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
      sectionName: http
  hostnames:
    - "app.example.com"
  rules:
    # 规则 1：API 路由，添加自定义头
    - matches:
        - path:
            type: PathPrefix
            value: /api/v2
      filters:
        - type: RequestHeaderModifier
          requestHeaderModifier:
            add:
              - name: X-API-Version
                value: "v2"
      backendRefs:
        - name: api-v2
          port: 8080

    # 规则 2：灰度发布，90% 流量到稳定版，10% 到金丝雀
    - matches:
        - path:
            type: PathPrefix
            value: /api/v1
      backendRefs:
        - name: api-v1-stable
          port: 8080
          weight: 90
        - name: api-v1-canary
          port: 8080
          weight: 10

    # 规则 3：旧路径重定向
    - matches:
        - path:
            type: Exact
            value: /old-page
      filters:
        - type: RequestRedirect
          requestRedirect:
            path:
              type: ReplaceFullPath
              replaceFullPath: /new-page
            statusCode: 301

    # 规则 4：默认后端
    - backendRefs:
        - name: frontend
          port: 3000
```

## 子文档导航

| 主题 | 文档 | 说明 |
|------|------|------|
| 匹配规则 | [matches/](./matches/README.md) | 路径、头部、查询参数、方法匹配 |
| 过滤器 | [filters/](./filters/README.md) | Gateway API 标准 Filter 与 Edgion 扩展 Plugin |
| 后端配置 | [backends/](./backends/README.md) | 多后端分流、权重、跨命名空间引用 |
| 弹性配置 | [resilience/](./resilience/) | 超时、重试、熔断等弹性策略 |
