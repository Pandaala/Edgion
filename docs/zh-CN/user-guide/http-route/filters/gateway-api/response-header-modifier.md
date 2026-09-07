# ResponseHeaderModifier

## 概述

`ResponseHeaderModifier` 在响应返回客户端之前修改 HTTP 响应头。它是标准 Gateway API 过滤器，在响应阶段运行，即上游响应头到达之后。使用它可以添加安全响应头、剥离泄露后端信息的响应头，或为响应打上网关元数据标记。

## 配置

```yaml
filters:
  - type: ResponseHeaderModifier
    responseHeaderModifier:
      set:
        - name: X-Powered-By
          value: Edgion
      add:
        - name: Cache-Control
          value: no-store
      remove:
        - Server
```

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `set` | Array\<Object\> | 无 | 每条 `{name, value}` 替换该响应头，覆盖所有已有值 |
| `add` | Array\<Object\> | 无 | 每条 `{name, value}` 追加一个值，保留已有值 |
| `remove` | Array\<String\> | 无 | 从响应中删除的响应头名称列表 |

三个操作按顺序应用：`set`，然后 `add`，然后 `remove`。Gateway API 配置要求每个不区分大小写的响应头名称只能执行一种操作，因此同等名称不能在一个列表中重复，也不能跨多个列表使用。

## 示例

### 示例 1：添加安全响应头并隐藏后端信息

添加严格传输安全响应头，并删除上游的 `Server` 标识。

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
        - type: ResponseHeaderModifier
          responseHeaderModifier:
            set:
              - name: Strict-Transport-Security
                value: "max-age=31536000"
            remove:
              - Server
      backendRefs:
        - name: api-service
          port: 8080
```

### 示例 2：多值响应头与 add

`add` 在不移除已有值的情况下追加。如果上游响应包含 `Vary: Accept-Encoding`，以下示例会向客户端返回 `Vary: Accept-Encoding` 和网关追加的 `Vary: Origin`。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: vary-route
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
        - type: ResponseHeaderModifier
          responseHeaderModifier:
            add:
              - name: Vary
                value: Origin
      backendRefs:
        - name: backend-service
          port: 8080
```

## 注意事项

- 响应头名称不区分大小写。
- 受支持的 Gateway API 配置会将每个不区分大小写的响应头名称分配给恰好一种操作，并且不包含同等名称的重复项。
- **无效输入的防御性行为：**如果非准入来源提供了重复项，网关会在 `set`、`add` 和 `remove` 各列表中仅保留第一个不区分大小写的同等项，然后按 `set` → `add` → `remove` 的顺序应用这些列表。此回退行为并不代表支持包含重复名称或跨操作同名项的配置。
- 值按字面量处理，不进行变量展开。
- 本过滤器在响应阶段运行；如需修改请求头，请使用 [RequestHeaderModifier](./request-header-modifier.md)。
- 如需重写状态码或进行更丰富的响应转换，请参阅 [ResponseRewrite 插件](../edgion-plugins/response-rewrite.md)。

## 相关文档

- [RequestHeaderModifier](./request-header-modifier.md)
- [ResponseRewrite 插件](../edgion-plugins/response-rewrite.md)
- [插件组合与执行顺序](../plugin-composition.md)
