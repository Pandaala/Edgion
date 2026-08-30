# HTTPRoute 匹配规则

匹配规则决定一条路由规则应用于哪些请求。规则的 `matches` 可以测试请求的路径、请求头、查询参数和 HTTP 方法。

## 多个 match 的组合方式

- **同一 match 内的条件为 AND 关系**：一个 match 块若同时设置了 `path`、`headers` 和 `method`，则这三个条件必须同时满足。
- **同一规则中的多个 match 为 OR 关系**：请求满足其中任意一个 match 块即匹配该规则。
- **最具体的 match 优先**：当多条规则同时匹配一个请求时，Edgion 选择最具体的那条——精确路径和更长的前缀优先于较短的前缀。
- **省略 `path` 等价于 `PathPrefix /`**：只设置 `headers`、`queryParams` 或 `method` 的 match 块作用于所有路径。该行为在所有配置中心后端（Kubernetes、File、Etcd）下一致。

```yaml
rules:
  - matches:
      # Match A：路径前缀为 /api、方法为 GET、携带 JWT 认证头（三者均为 AND）
      - path:
          type: PathPrefix
          value: /api
        method: GET
        headers:
          - name: X-Auth-Type
            value: jwt
      # Match B：路径前缀为 /public（与 Match A 为 OR）
      - path:
          type: PathPrefix
          value: /public
    backendRefs:
      - name: api-service
        port: 8080
```

## 大小写敏感性

| 维度 | 大小写行为 |
|------|-----------|
| 路径 | 大小写敏感 |
| 请求头名称 | 大小写不敏感（HTTP 头名称本身大小写不敏感） |
| 请求头值 | 大小写敏感 |
| 查询参数名称和值 | 大小写敏感 |
| 方法 | 大写 token（`GET`、`POST` 等） |

## 子文档

- [路径匹配](./path.md)
- [请求头匹配](./headers.md)
- [查询参数匹配](./query-params.md)
- [HTTP 方法匹配](./method.md)
