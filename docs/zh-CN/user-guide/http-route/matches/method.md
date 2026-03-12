# HTTP 方法匹配

基于 HTTP 请求方法进行路由匹配。

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

## 示例

### 示例 1: RESTful 路由

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: restful-routing
spec:
  parentRefs:
    - name: my-gateway
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
    # POST/PUT/DELETE /users -> 写入服务
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

### 示例 2: 只读网关

```yaml
rules:
  - matches:
      - method: GET
      - method: HEAD
      - method: OPTIONS
    backendRefs:
      - name: backend
        port: 8080
```

只允许只读请求通过。

## 相关文档

- [路径匹配](./path.md)
- [请求头匹配](./headers.md)
