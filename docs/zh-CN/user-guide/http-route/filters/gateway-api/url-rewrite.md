# URLRewrite

## 概述

`URLRewrite` 在请求转发到上游之前重写请求的主机名和/或路径。它是标准 Gateway API 过滤器，在请求阶段运行。与 [RequestRedirect](./request-redirect.md) 不同，重写对客户端透明：请求以新主机名/路径代理到后端，不会向客户端返回重定向响应。

## 配置

```yaml
filters:
  - type: URLRewrite
    urlRewrite:
      hostname: internal.example.com
      path:
        type: ReplacePrefixMatch
        replacePrefixMatch: /v2
```

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `hostname` | String | 无 | 替换发往上游的请求 `Host` 头 |
| `path` | Object | 无 | 应用的路径重写；见下方子字段说明 |

### `path` 子字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `path.type` | Enum | 无 | 允许值：`ReplaceFullPath` \| `ReplacePrefixMatch` |
| `path.replaceFullPath` | String | 无 | 当 `type: ReplaceFullPath` 时使用；替换整个路径 |
| `path.replacePrefixMatch` | String | 无 | 当 `type: ReplacePrefixMatch` 时使用；仅替换规则 `PathPrefix` 匹配的部分 |

## 示例

### 示例 1：重写上游主机名

将请求转发给需要特定内部 `Host` 的后端，路径不变。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: host-rewrite
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - "api.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /
      filters:
        - type: URLRewrite
          urlRewrite:
            hostname: internal.example.com
      backendRefs:
        - name: api-service
          port: 8080
```

### 示例 2：ReplacePrefixMatch 与 ReplaceFullPath 的区别

`ReplacePrefixMatch` 剥离匹配的前缀并替换为新前缀，保留路径的其余部分（因此 `/api/users/42` 变为 `/v2/users/42`）。`ReplaceFullPath` 则完全丢弃原始路径。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: path-rewrite
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    # /api/users/42  ->  /v2/users/42
    - matches:
        - path:
            type: PathPrefix
            value: /api
      filters:
        - type: URLRewrite
          urlRewrite:
            path:
              type: ReplacePrefixMatch
              replacePrefixMatch: /v2
      backendRefs:
        - name: api-service
          port: 8080
    # /healthz  ->  /status（完整替换）
    - matches:
        - path:
            type: PathPrefix
            value: /healthz
      filters:
        - type: URLRewrite
          urlRewrite:
            path:
              type: ReplaceFullPath
              replaceFullPath: /status
      backendRefs:
        - name: api-service
          port: 8080
```

## 注意事项

- `URLRewrite` 与 [RequestRedirect](./request-redirect.md) 在同一规则中**互斥**。同时声明两者的规则将被标记为 `PartiallyInvalid` 并被禁用，但 HTTPRoute 中其他有效规则继续生效。
- 重写值按字面量处理；路径参数（`:name`）和模板变量不会展开。如需动态、变量驱动的重写，请使用 [ProxyRewrite 插件](../edgion-plugins/proxy-rewrite.md)。
- `ReplacePrefixMatch` 仅在 `PathPrefix` 匹配时有意义；它精确地替换匹配的前缀片段。

## 相关文档

- [RequestRedirect](./request-redirect.md)
- [ProxyRewrite 插件](../edgion-plugins/proxy-rewrite.md)
- [RequestHeaderModifier](./request-header-modifier.md)
- [插件组合与执行顺序](../plugin-composition.md)
