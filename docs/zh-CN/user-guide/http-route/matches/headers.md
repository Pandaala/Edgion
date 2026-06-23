# 请求头匹配

请求头匹配根据 HTTP 请求头对请求进行路由，支持 `Exact` 和 `RegularExpression` 两种匹配类型。

## 匹配类型

### Exact

```yaml
matches:
  - headers:
      - name: X-Env
        type: Exact
        value: production
```

当 `X-Env` 头的值恰好为 `production` 时匹配。

### RegularExpression

```yaml
matches:
  - headers:
      - name: X-Request-ID
        type: RegularExpression
        value: "^[a-f0-9-]{36}$"
```

当请求头的值满足正则表达式时匹配。

## 配置参考

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | String | **必填** | 请求头名称。大小写不敏感 |
| `type` | Enum | `Exact` | 匹配类型。允许值：`Exact` \| `RegularExpression` |
| `value` | String | **必填** | 要匹配的值。大小写敏感 |

## 请求头条件的组合方式

- 同一个 `headers` 列表中的多个条目为 **AND 关系**——每个请求头条件都必须满足。
- 请求头条件与同一 match 块中的 `path`、`queryParams` 和 `method` 也是 AND 关系。
- 请求头**名称**大小写不敏感（符合 HTTP 规范）；请求头**值**的匹配区分大小写。

## 示例

### 示例 1：基于请求头的灰度路由

携带 `X-Env: canary` 的请求转发到金丝雀后端；其他请求落到稳定版后端。因为金丝雀规则的匹配更具体，所以会优先被评估：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: env-routing
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - headers:
            - name: X-Env
              value: canary
      backendRefs:
        - name: app-canary
          port: 8080
    - matches:
        - path:
            type: PathPrefix
            value: /
      backendRefs:
        - name: app-stable
          port: 8080
```

### 示例 2：多个条件 AND 组合

请求仅在路径前缀**且**两个请求头条件均满足时才匹配：

```yaml
matches:
  - path:
      type: PathPrefix
      value: /api
    headers:
      - name: X-Auth-Type
        value: jwt
      - name: X-Version
        value: "2"
```

### 示例 3：两个请求头值的 OR 匹配

两个独立的 match 块为 OR 关系，满足其中任意一个头值即可匹配：

```yaml
matches:
  - headers:
      - name: X-Tier
        value: gold
  - headers:
      - name: X-Tier
        value: platinum
```

## 相关文档

- [路径匹配](./path.md)
- [查询参数匹配](./query-params.md)
- [HTTP 方法匹配](./method.md)
