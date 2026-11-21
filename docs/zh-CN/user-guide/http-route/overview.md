# HTTPRoute 总览

HTTPRoute 是 Gateway API 中用于定义 HTTP/HTTPS 流量路由规则的核心资源，根据匹配条件将请求映射到后端服务，执行过滤器，并将流量分发到多个后端。

## 基本结构

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: example-route
  namespace: production
spec:
  # 绑定到哪个 Gateway listener
  parentRefs:
    - name: main-gateway
      sectionName: http
  # 匹配的域名（Host 头）
  hostnames:
    - "app.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api
      backendRefs:
        - name: api-service
          port: 8080
```

关键字段说明：

| 字段 | 说明 |
|------|------|
| `parentRefs` | 将路由绑定到一个或多个 Gateway listener |
| `hostnames` | 匹配请求的 Host 头（精确或 `*.` 通配符） |
| `rules` | 路由规则列表，每条包含 matches、filters 和 backendRefs |

## 核心概念

### Hostnames

`hostnames` 最多接受 16 个小写 DNS 名称，也支持 `api` 这样的短名称。`*.example.com` 同时匹配 `api.example.com` 和 `a.b.example.com`，但不匹配 `example.com`。IP 地址、格式错误的名称和单独的 `*` 均无效；任一项无效会拒绝整个列表。省略或使用空列表表示路由不限制 hostname，并不跳过监听器校验。

匹配的声明域名按精确名称、最长通配符后缀的顺序考虑，最后才是未限制 hostname 的路由。监听器交集只限制可用范围，不提升通配符路由的优先级。路径或其他匹配条件不满足时，可继续检查其他合格候选；更具体的监听器不会因为没有匹配路由而被绕过。这些路由通配符规则不改变证书匹配规则。

### ParentRefs

`parentRefs` 将路由附加到 Gateway listener。Controller（控制平面）会将路由的 hostname 与 listener 的 hostname 取交集，仅将路由绑定到匹配的 listener。

HTTPRoute 和父 Gateway 可以按任意顺序创建。如果路由先到，Controller 会在 Gateway 就绪后重新解析路由。

在 FileSystem 模式下，请显式填写 `metadata.namespace` 并使用匹配的文件名，例如 `default/HTTPRoute_default_my-route.yaml`。命名空间缺失时，Controller 会在该文件的状态中记录 `ParseError`。

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | String | **必填** | Gateway 名称 |
| `namespace` | String | 否 | Gateway 命名空间；跨命名空间需要 `ReferenceGrant` |
| `sectionName` | String | 否 | Gateway 上特定 listener 的名称 |
| `port` | Integer | 否 | 特定 listener 的端口 |

一个 HTTPRoute 可以绑定多个 parentRefs，实现跨 Gateway 共享路由规则。跨命名空间详情见 [后端配置](./backends/README.md)。

同时设置 `sectionName` 和 `port` 时，listener 必须同时满足两项约束。多个 Gateway 共用物理端口不会扩大 parentRef 的绑定范围：引用 Gateway A 的 8080 端口，不会因为另一个 parentRef 而允许匹配 Gateway A 的 8081 端口。

### Rules

`rules` 中的每个条目都是一个独立的路由决策单元，包含匹配条件、过滤器和后端引用。当一个请求匹配多条规则时，最具体的匹配优先（更长的路径前缀和精确路径优先于较短的前缀）。

### Matches

`matches` 定义请求必须满足的条件，支持路径、请求头、查询参数和 HTTP 方法匹配。同一 match 内的条件为 AND 关系；同一规则中的多个 match 之间为 OR 关系。详见 [匹配规则](./matches/README.md)。

### Filters / Plugins

`filters` 在请求到达后端之前或响应返回客户端之前执行处理逻辑：

- **Edgion 已处理的 Gateway API Filter**：`RequestHeaderModifier`、`ResponseHeaderModifier`、`RequestRedirect`、`URLRewrite`、`RequestMirror` 和原生 `CORS`。上游 `ExternalAuth` 的 `backendRef`／`protocol` 形式会被拒绝；需要该功能时请使用 Edgion 独立的 ForwardAuth 插件。
- **Edgion 扩展 Plugin**：通过 `ExtensionRef` 引用 `EdgionPlugins` 资源（限流、认证、DSL 等）。

`URLRewrite` 和 `RequestRedirect` 在同一规则内互斥。详见 [过滤器](./filters/README.md)。

### BackendRefs

`backendRefs` 列出接收匹配流量的后端服务：

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | String | **必填** | Service 名称 |
| `port` | Integer | 否 | 目标端口 |
| `weight` | Integer | `1` | 流量权重，用于加权轮询的比例计算 |
| `namespace` | String | 否 | Service 命名空间；跨命名空间需要 `ReferenceGrant` |

多个加权后端可实现灰度发布和蓝绿部署。详见 [后端配置](./backends/README.md) 和 [负载均衡算法](./lb-algorithms.md)。

## 完整示例

以下 HTTPRoute 跨多条规则，覆盖路径匹配、头部修改、流量分流和重定向：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: app-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: http
  hostnames:
    - "app.example.com"
  rules:
    # 规则 1：API v2 路由，添加自定义头
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
        - name: api-v2-service
          port: 8080

    # 规则 2：灰度发布——90% 到稳定版，10% 到金丝雀
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

    # 规则 3：重定向已废弃的路径
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

    # 规则 4：默认后端，配置弹性策略
    - timeouts:
        request: "30s"
        backendRequest: "10s"
      retry:
        attempts: 3
        backoff: "1s"
        codes:
          - 502
          - 503
          - 504
      backendRefs:
        - name: frontend-service
          port: 8080
```

## 子文档导航

| 主题 | 文档 | 说明 |
|------|------|------|
| 匹配规则 | [matches/](./matches/README.md) | 路径、请求头、查询参数、方法匹配 |
| 过滤器 | [filters/](./filters/README.md) | Gateway API 标准 Filter 与 Edgion 扩展 Plugin |
| 后端配置 | [backends/](./backends/README.md) | 加权流量分流、跨命名空间引用、健康检查 |
| 负载均衡 | [lb-algorithms.md](./lb-algorithms.md) | RoundRobin、ConsistentHash、LeastConnection 和 EWMA 算法 |
| 弹性配置 | [resilience/](./resilience/README.md) | 超时、重试和会话保持 |
