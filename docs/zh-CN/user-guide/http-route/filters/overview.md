# HTTPRoute 过滤器

过滤器允许你在请求流经 HTTPRoute 规则时对其进行修改或处理——重写请求头、重定向、镜像流量、执行认证、限流等。Edgion 支持两大类过滤器：**标准 Gateway API 过滤器**和通过 `ExtensionRef` 挂载的 **Edgion 扩展插件**。

## 两大过滤器家族

### 标准 Gateway API 过滤器

这些过滤器是 Gateway API 规范的组成部分，配置在 `rules[].filters`（或 `rules[].backendRefs[].filters`）中，通过 `type` 字段区分：

| Filter `type` | 功能 | 阶段 |
|---------------|------|------|
| `RequestHeaderModifier` | 在转发前设置、添加或删除请求头 | 请求阶段 |
| `ResponseHeaderModifier` | 在返回前设置、添加或删除响应头 | 响应阶段 |
| `RequestRedirect` | 返回 301/302/303/307/308 重定向，不代理到后端 | 请求阶段（终止） |
| `URLRewrite` | 在转发前重写请求主机名和/或路径 | 请求阶段 |
| `RequestMirror` | 将部分流量复制到第二个后端（fire-and-forget） | 请求阶段（非阻塞） |

注入请求头的最简示例：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: example-route
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
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set:
              - name: X-Gateway
                value: edgion
      backendRefs:
        - name: api-service
          port: 8080
```

### Edgion 扩展插件（通过 ExtensionRef）

超出五个标准过滤器范围的能力——CORS、JWT/Basic/Key/LDAP/HMAC 认证、限流、IP 限制、Mock、代理重写、DSL 脚本等——均通过 `EdgionPlugins` 自定义资源提供。通过标准 Gateway API 的 `ExtensionRef` 过滤器将其挂载到路由：

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
```

完整插件目录请见 [Edgion 扩展插件](./edgion-plugins/README.md)。

## 过滤器作用域

过滤器可在两个作用域声明：

| 作用域 | 位置 | 适用范围 |
|--------|------|----------|
| 规则级 | `rules[].filters` | 匹配该规则的所有请求 |
| BackendRef 级 | `rules[].backendRefs[].filters` | 仅路由到该后端的请求 |

BackendRef 级过滤器适合金丝雀发布场景——例如只在发往金丝雀后端的请求上添加 `X-Canary` 请求头。

从 `backendRefs[].filters` 引用的 `EdgionPlugins` 不能包含已启用的
`DirectEndpoint`、`DynamicInternalUpstream`、`DynamicExternalUpstream`、`RegionRoute` 或 `Canary`
请求插件。这些插件必须在选定 BackendRef 之前选择上游；Controller 会将相应规则标为无效，
Gateway 运行时也会拒绝该引用。请将上游选择插件挂载到规则、Gateway 或全局作用域。

规则级 `RequestRedirect` 是终止型路由动作，其规则必须省略 `backendRefs`；
Kubernetes CRD 会拒绝同时包含两者的规则。

## 互斥限制：URLRewrite 与 RequestRedirect

`URLRewrite` 和 `RequestRedirect` **互斥**，不能出现在同一个规则中。冲突检测分三个层面：

1. 两者同时出现在 `rule.filters` 中。
2. 两者同时出现在同一个 `backendRef.filters` 中。
3. 一个在 `rule.filters`，另一个在 `backendRef.filters`（跨层）。

违反此规则的规则将被标记为 `PartiallyInvalid`：该规则被禁用，但 HTTPRoute 中其他有效规则继续生效。

## 执行顺序

当一个请求从全局配置、路由规则和 BackendRef 分别获取过滤器时，这些过滤器按照既定的优先级和代理阶段运行。完整模型（含 `ExtensionRef` 插件与标准过滤器的交织方式）请见 [插件组合与执行顺序](./plugin-composition.md)。

## 相关文档

- [插件组合与执行顺序](./plugin-composition.md)
- [HTTPRoute 总览](../overview.md)
- [RequestHeaderModifier](./gateway-api/request-header-modifier.md)
- [Edgion 扩展插件](./edgion-plugins/README.md)
