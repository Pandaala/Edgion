# 路径匹配

HTTPRoute 支持三种路径匹配类型。

## 匹配类型

### Exact - 精确匹配

```yaml
matches:
  - path:
      type: Exact
      value: /api/users
```

只匹配 `/api/users`，不匹配 `/api/users/` 或 `/api/users/123`。

### PathPrefix - 前缀匹配

```yaml
matches:
  - path:
      type: PathPrefix
      value: /api
```

匹配所有以 `/api` 开头的路径：
- ✓ `/api`
- ✓ `/api/`
- ✓ `/api/users`
- ✗ `/apiV2`

### RegularExpression - 正则匹配

```yaml
matches:
  - path:
      type: RegularExpression
      value: "^/api/v[0-9]+/.*"
```

使用正则表达式匹配路径。

## 配置参考

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| type | string | | PathPrefix | 匹配类型 |
| value | string | ✓ | | 匹配值 |

## 示例

### 示例 1: API 版本路由

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-versioning
spec:
  parentRefs:
    - name: my-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/v1
      backendRefs:
        - name: api-v1
          port: 8080
    - matches:
        - path:
            type: PathPrefix
            value: /api/v2
      backendRefs:
        - name: api-v2
          port: 8080
```

### 示例 2: 静态资源与 API 分离

```yaml
rules:
  - matches:
      - path:
          type: PathPrefix
          value: /static
    backendRefs:
      - name: static-server
        port: 80
  - matches:
      - path:
          type: PathPrefix
          value: /api
    backendRefs:
      - name: api-server
        port: 8080
```

## 相关文档

- [请求头匹配](./headers.md)
- [查询参数匹配](./query-params.md)
- [HTTP 方法匹配](./method.md)
