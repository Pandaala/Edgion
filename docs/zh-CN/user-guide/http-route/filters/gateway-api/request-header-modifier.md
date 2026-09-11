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
- 名称必须是 1–256 字节的合法 HTTP 头名称；值必须是 1–4096 个字符的合法 HTTP 头值。CR、LF、NUL 等非法控制字符会被拒绝。
- `set`、`add`、`remove` 各最多包含 16 项。同一个名称（不区分大小写）不能出现在多种操作中。对于通过非 Kubernetes 原生准入入口提供的同列表等价名称，仅使用第一项，但每一项仍必须通过校验。操作顺序为 `set` → `add` → `remove`。
- HTTPRoute 和 GRPCRoute 原生过滤器的 `type` 必须与对应配置字段精确配对。同一个规则或后端过滤器列表内，请求头、响应头修改器分别只能出现一次。规则级与后端级修改器属于不同列表。
- 校验同样适用于 EdgionPlugins 及文件/etcd 配置。无效原生修改器会使所属规则无效，正确的兄弟规则仍可使用，状态会报告配置错误。若非法配置进入 Gateway 构造阶段，对应运行时将以通用配置错误拒绝请求，不再静默跳过修改器。
- 值按字面量处理；路径参数（`:name`）和模板变量**不会**在此展开。如需动态替换，请使用 [ProxyRewrite 插件](../edgion-plugins/proxy-rewrite.md)。
- AI 提供商的后端过滤器应在各列表内使用唯一名称；目前不保证上述首项保留行为。
- 如需修改响应头，请使用 [ResponseHeaderModifier](./response-header-modifier.md)。

## 相关文档

- [ResponseHeaderModifier](./response-header-modifier.md)
- [URLRewrite](./url-rewrite.md)
- [ProxyRewrite 插件](../edgion-plugins/proxy-rewrite.md)
- [插件组合与执行顺序](../plugin-composition.md)
