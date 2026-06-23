# 查询参数匹配

查询参数匹配根据 URL 查询字符串对请求进行路由，支持 `Exact` 和 `RegularExpression` 两种匹配类型。

## 匹配类型

### Exact

```yaml
matches:
  - queryParams:
      - name: version
        type: Exact
        value: "2"
```

匹配 `?version=2`。

### RegularExpression

```yaml
matches:
  - queryParams:
      - name: id
        type: RegularExpression
        value: "^[0-9]+$"
```

当参数值满足正则表达式时匹配。

## 配置参考

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | String | **必填** | 查询参数名称。大小写敏感 |
| `type` | Enum | `Exact` | 匹配类型。允许值：`Exact` \| `RegularExpression` |
| `value` | String | **必填** | 要匹配的值。大小写敏感 |

## 查询参数条件的组合方式

- 同一个 `queryParams` 列表中的多个条目为 **AND 关系**——每个条件都必须满足。
- 查询参数条件与同一 match 块中的 `path`、`headers` 和 `method` 也是 AND 关系。
- 参数名称和值均区分大小写。

## 示例

### 示例 1：按 API 版本参数路由

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-version-param
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - queryParams:
            - name: api_version
              value: "v2"
      backendRefs:
        - name: api-v2-service
          port: 8080
    - backendRefs:
        - name: api-v1-service
          port: 8080
```

### 示例 2：多个参数 AND 组合

仅当 `region=eu` 和 `tier=premium` 同时存在时才匹配：

```yaml
matches:
  - queryParams:
      - name: region
        value: eu
      - name: tier
        value: premium
```

## 相关文档

- [路径匹配](./path.md)
- [请求头匹配](./headers.md)
- [HTTP 方法匹配](./method.md)
