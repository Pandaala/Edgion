# 查询参数匹配

基于 URL 查询参数进行路由匹配。

## 匹配类型

### Exact - 精确匹配

```yaml
matches:
  - queryParams:
      - name: version
        type: Exact
        value: "2"
```

匹配 `?version=2`。

### RegularExpression - 正则匹配

```yaml
matches:
  - queryParams:
      - name: id
        type: RegularExpression
        value: "^[0-9]+$"
```

## 配置参考

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| name | string | ✓ | | 参数名称 |
| type | string | | Exact | 匹配类型 |
| value | string | ✓ | | 匹配值 |

## 示例

### 示例 1: API 版本选择

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-version-param
spec:
  parentRefs:
    - name: my-gateway
  rules:
    - matches:
        - queryParams:
            - name: api_version
              value: "v2"
      backendRefs:
        - name: api-v2
          port: 8080
    - backendRefs:
        - name: api-v1
          port: 8080
```

## 相关文档

- [路径匹配](./path.md)
- [请求头匹配](./headers.md)
