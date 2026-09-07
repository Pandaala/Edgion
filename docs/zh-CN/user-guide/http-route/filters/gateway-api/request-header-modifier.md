# RequestHeaderModifier

## 概述

`RequestHeaderModifier` 在请求转发到上游之前修改 HTTP 请求头。它是标准 Gateway API 过滤器，在请求阶段运行。使用它可以注入路由元数据、剥离客户端不应控制的内部请求头，或对请求头值进行规范化。

## 配置

```yaml
filters:
  - type: RequestHeaderModifier
    requestHeaderModifier:
      set:
        - name: X-Gateway
          value: edgion
      add:
        - name: X-Forwarded-Env
          value: production
      remove:
        - X-Internal-Token
```

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `set` | Array\<Object\> | 无 | 每条 `{name, value}` 替换该请求头，覆盖所有已有值 |
| `add` | Array\<Object\> | 无 | 每条 `{name, value}` 追加一个值，保留已有值 |
| `remove` | Array\<String\> | 无 | 从请求中删除的请求头名称列表 |

三个操作按顺序应用：`set`，然后 `add`，然后 `remove`。Gateway API 配置要求每个不区分大小写的请求头名称只能执行一种操作，因此同等名称不能在一个列表中重复，也不能跨多个列表使用。

## 示例

### 示例 1：注入并剥离请求头

为每个请求打上网关标识，并删除客户端不应自行设置的请求头。

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
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set:
              - name: X-Gateway
                value: edgion
            remove:
              - X-Internal-Token
      backendRefs:
        - name: api-service
          port: 8080
```

### 示例 2：向已有的多值请求头追加值

`set` 完全替换请求头，而 `add` 保留传入请求中已有的值。如果客户端发送 `X-Trace: client`，以下示例会向上游转发 `X-Trace: client` 和 `X-Trace: edge`。任何传入的 `Accept-Encoding` 值也会被替换为单个值 `gzip`。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: header-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /
      filters:
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set:
              - name: Accept-Encoding
                value: gzip
            add:
              - name: X-Trace
                value: edge
      backendRefs:
        - name: backend-service
          port: 8080
```

## 注意事项

- 请求头名称不区分大小写。
- 受支持的 Gateway API 配置会将每个不区分大小写的请求头名称分配给恰好一种操作，并且不包含同等名称的重复项。
- **无效输入的防御性行为：**如果非准入来源提供了重复项，网关会在 `set`、`add` 和 `remove` 各列表中仅保留第一个不区分大小写的同等项，然后按 `set` → `add` → `remove` 的顺序应用这些列表。此回退行为并不代表支持包含重复名称或跨操作同名项的配置。
- 值按字面量处理；路径参数（`:name`）和模板变量**不会**在此展开。如需动态替换，请使用 [ProxyRewrite 插件](../edgion-plugins/proxy-rewrite.md)。
- 如需修改响应头，请使用 [ResponseHeaderModifier](./response-header-modifier.md)。

## 相关文档

- [ResponseHeaderModifier](./response-header-modifier.md)
- [URLRewrite](./url-rewrite.md)
- [ProxyRewrite 插件](../edgion-plugins/proxy-rewrite.md)
- [插件组合与执行顺序](../plugin-composition.md)
