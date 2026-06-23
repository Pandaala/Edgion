# 路径匹配

路径匹配根据 URL 路径对请求进行路由。HTTPRoute 支持三种匹配类型：`Exact`、`PathPrefix` 和 `RegularExpression`。

## 匹配类型

### Exact

```yaml
matches:
  - path:
      type: Exact
      value: /api/users
```

只匹配 `/api/users`，不匹配 `/api/users/` 或 `/api/users/123`。

### PathPrefix

```yaml
matches:
  - path:
      type: PathPrefix
      value: /api
```

匹配**路径段**以该前缀开头的路径：

- 匹配：`/api`、`/api/`、`/api/users`
- 不匹配：`/apiV2`（前缀必须在 `/` 路径段边界对齐，不是子字符串匹配）

### RegularExpression

```yaml
matches:
  - path:
      type: RegularExpression
      value: "^/api/v[0-9]+/.*"
```

使用正则表达式匹配路径。适用于前缀匹配无法表达的模式，例如版本范围匹配。

## 配置参考

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `type` | Enum | `PathPrefix` | 匹配类型。允许值：`Exact` \| `PathPrefix` \| `RegularExpression` |
| `value` | String | `/`（`PathPrefix` 时） | 要匹配的路径或模式。大小写敏感 |

对于 `Exact` 和 `PathPrefix`，配置的值在加载时会被规范化（percent 编码的斜杠被解码，点段和重复斜杠被折叠），从而与规范化后的请求路径匹配。`RegularExpression` 的值保持原样。

## 命名路径参数

> **🔌 Edgion extension**
> 
> `:name` 路径参数语法是 Edgion 对 Gateway API 路径值的扩展。

对于 `Exact` 和 `PathPrefix` 类型，`value` 可以使用 `:name` 包含命名路径参数：

| 模式 | 行为 |
|------|------|
| `/users/:id` | 将第二个路径段捕获为 `id` |
| `/api/:version/users/:uid` | 捕获多个参数 |
| `/data/::literal` | `::` 转义为字面量 `:`；该路径段是 `:literal`，而不是参数 |

捕获的值仅在运行时通过以下方式可用：

- DSL 插件内置函数 `req.path_param("name")`。
- `ProxyRewrite` 插件中的 `$name` 替换（URI 和 Header 值）。

标准 Gateway API filter（`URLRewrite`、`RequestRedirect`、`RequestHeaderModifier`、`ResponseHeaderModifier`）将其值字符串视为字面量，不展开 `:name`。`RegularExpression` 路径不参与 `:name` 捕获——在 `ProxyRewrite` 中使用正则捕获组（`$1`–`$9`）代替。

## 优先级

当多条规则匹配同一请求时，最具体的路径优先：

- `Exact` 匹配比任何前缀匹配更具体。
- 更长的 `PathPrefix` 优先于更短的（`/api/v1` 优先于 `/api`）。

## 示例

### 示例 1：API 版本路由

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-versioning
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/v1
      backendRefs:
        - name: api-v1-service
          port: 8080
    - matches:
        - path:
            type: PathPrefix
            value: /api/v2
      backendRefs:
        - name: api-v2-service
          port: 8080
```

### 示例 2：精确路径覆盖前缀

精确匹配 `/api/health` 的规则优先于更宽泛的 `/api` 前缀，因为精确匹配更具体：

```yaml
rules:
  - matches:
      - path:
          type: Exact
          value: /api/health
    backendRefs:
      - name: health-service
        port: 8080
  - matches:
      - path:
          type: PathPrefix
          value: /api
    backendRefs:
      - name: api-service
        port: 8080
```

### 示例 3：版本化路径的正则表达式

```yaml
rules:
  - matches:
      - path:
          type: RegularExpression
          value: "^/api/v[0-9]+/users/[0-9]+$"
    backendRefs:
      - name: user-service
        port: 8080
```

## 相关文档

- [请求头匹配](./headers.md)
- [查询参数匹配](./query-params.md)
- [HTTP 方法匹配](./method.md)
