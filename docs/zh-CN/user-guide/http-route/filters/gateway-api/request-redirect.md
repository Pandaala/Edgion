# RequestRedirect

## 概述

`RequestRedirect` 返回 HTTP 重定向响应（301、302、303、307 或 308）而不将请求代理到后端。它是标准 Gateway API 过滤器，在请求阶段运行。由于会终止请求，上游永远不会被访问。可用于强制 HTTPS、将流量迁移到新主机名，或在迁移过程中重写路径。

## 配置

```yaml
filters:
  - type: RequestRedirect
    requestRedirect:
      scheme: https
      hostname: new.example.com
      port: 443
      statusCode: 301
      path:
        type: ReplaceFullPath
        replaceFullPath: /new-path
```

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `scheme` | Enum | 无 | 重定向协议方案。允许值：`http` \| `https` |
| `hostname` | String | 无 | 重定向目标主机名；省略时保留原始主机名 |
| `port` | Integer | 无 | 重定向目标端口；省略时使用协议默认端口 |
| `statusCode` | Enum | `302` | 重定向状态码。允许值：`301` \| `302` \| `303` \| `307` \| `308` |
| `path` | Object | 无 | 应用于重定向目标的路径重写；见下方子字段说明 |

### `path` 子字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `path.type` | Enum | 无 | 允许值：`ReplaceFullPath` \| `ReplacePrefixMatch` |
| `path.replaceFullPath` | String | 无 | 当 `type: ReplaceFullPath` 时使用；替换整个路径 |
| `path.replacePrefixMatch` | String | 无 | 当 `type: ReplacePrefixMatch` 时使用；仅替换规则 `PathPrefix` 匹配的部分 |

## 示例

### 示例 1：强制 HTTPS

使用永久 301 将所有纯 HTTP 流量重定向到 HTTPS，保留原始主机名和路径。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: https-redirect
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: http
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /
      filters:
        - type: RequestRedirect
          requestRedirect:
            scheme: https
            port: 443
            statusCode: 301
```

### 示例 2：切换主机名并重写路径前缀

使用临时 302 将旧主机名上的 `/old-api/...` 重定向到新主机名的 `/v2/...`。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: migrate-redirect
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - "legacy.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /old-api
      filters:
        - type: RequestRedirect
          requestRedirect:
            hostname: api.example.com
            statusCode: 302
            path:
              type: ReplacePrefixMatch
              replacePrefixMatch: /v2
```

## 注意事项

- `RequestRedirect` 与 [URLRewrite](./url-rewrite.md) 在同一规则中**互斥**。同时声明两者的规则将被标记为 `PartiallyInvalid` 并被禁用，但 HTTPRoute 中其他有效规则继续生效。
- 规则级 `RequestRedirect` 是终止型过滤器，不能与同一规则中的
  `backendRefs` 组合；Kubernetes CRD 会拒绝这种配置。
- 由于过滤器会终止请求，其后声明的请求阶段插件不会执行，后端也不会被访问。
- `ReplacePrefixMatch` 仅替换规则 `PathPrefix` 匹配的那部分路径，路径的其余部分得以保留。
- 原始查询字符串始终会被保留并追加到重定向的 `Location` URL 上。例如，`http://app.example.com/login?returnTo=/dashboard` 会重定向到 `https://app.example.com/login?returnTo=/dashboard`。Gateway API 重定向过滤器没有 query 修改器，因此无论是否配置了路径修改器，客户端的 query 始终会在重定向中保留。

## 相关文档

- [URLRewrite](./url-rewrite.md)
- [RequestHeaderModifier](./request-header-modifier.md)
- [HTTP 转 HTTPS 重定向（Gateway 级）](../../../../ops-guide/gateway/http-to-https-redirect.md)
- [插件组合与执行顺序](../plugin-composition.md)
