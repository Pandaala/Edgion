# HTTP 方法匹配

方法匹配根据 HTTP 方法对请求进行路由。

## 配置

```yaml
matches:
  - method: GET
```

支持的方法：

- `GET`
- `POST`
- `PUT`
- `DELETE`
- `PATCH`
- `HEAD`
- `OPTIONS`
- `CONNECT`

## 配置参考

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `method` | Enum | 无 | 要匹配的 HTTP 方法。使用大写方法 token |

## 方法条件的组合方式

- 一个 match 块中最多包含一个 `method`，与同一块中的 `path`、`headers` 和 `queryParams` 为 AND 关系。
- 若要匹配多个方法，将它们作为独立的 match 块列出；同一规则中的多个 match 之间为 OR 关系。

## 示例

### 示例 1：读写分离

`GET /users` 转发到查询服务；`POST`、`PUT` 和 `DELETE` 转发到命令服务。每个方法都是独立的 OR match 块：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: restful-routing
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    # GET /users -> 查询服务
    - matches:
        - path:
            type: PathPrefix
            value: /users
          method: GET
      backendRefs:
        - name: user-query-service
          port: 8080
    # POST/PUT/DELETE /users -> 命令服务
    - matches:
        - path:
            type: PathPrefix
            value: /users
          method: POST
        - path:
            type: PathPrefix
            value: /users
          method: PUT
        - path:
            type: PathPrefix
            value: /users
          method: DELETE
      backendRefs:
        - name: user-command-service
          port: 8080
```

### 示例 2：只读网关

只允许只读方法通过（三个 OR match 块）：

```yaml
rules:
  - matches:
      - method: GET
      - method: HEAD
      - method: OPTIONS
    backendRefs:
      - name: backend-service
        port: 8080
```

## 相关文档

- [路径匹配](./path.md)
- [请求头匹配](./headers.md)
- [查询参数匹配](./query-params.md)
