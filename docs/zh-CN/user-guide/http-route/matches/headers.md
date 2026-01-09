# 请求头匹配

基于 HTTP 请求头进行路由匹配。

## 匹配类型

### Exact - 精确匹配

```yaml
matches:
  - headers:
      - name: X-Env
        type: Exact
        value: production
```

### RegularExpression - 正则匹配

```yaml
matches:
  - headers:
      - name: X-Request-ID
        type: RegularExpression
        value: "^[a-f0-9-]{36}$"
```

## 配置参考

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| name | string | ✓ | | 请求头名称 |
| type | string | | Exact | 匹配类型 |
| value | string | ✓ | | 匹配值 |

## 示例

### 示例 1: 环境路由

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: env-routing
spec:
  parentRefs:
    - name: my-gateway
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

### 示例 2: 多条件组合

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

同时满足路径前缀和两个请求头才匹配。

## 相关文档

- [路径匹配](./path.md)
- [查询参数匹配](./query-params.md)
