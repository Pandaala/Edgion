# GRPCRoute 匹配规则

GRPCRoute 按 gRPC 服务名和方法名匹配入站调用，也可选地按 gRPC 元数据（headers）匹配。当**同一个 `matches` 条目内的所有条件**均满足时，规则触发；列出多个 `matches` 条目相当于 OR 关系。

## 匹配类型

| 匹配类型 | 说明 |
|---------|------|
| 方法（`Exact`） | 对 gRPC `service` 和/或 `method` 名称做精确匹配 |
| 方法（`RegularExpression`） | 对 `service` 和/或 `method` 做正则表达式匹配 |
| Headers（`Exact`） | 对 gRPC 元数据的键/值（请求头）做精确匹配 |
| Headers（`RegularExpression`） | 对元数据值做正则表达式匹配 |

gRPC 请求路径的格式为 `/<package>.<Service>/<Method>`。在方法匹配中，`service` 是完整的 `<package>.<Service>`，`method` 是 RPC 名称。

## 方法匹配

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: user-service-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - method:
            type: Exact
            service: user.UserService
            method: GetUser
      backendRefs:
        - name: user-service
          port: 9000
```

省略 `method` 时匹配指定 `service` 的所有 RPC；省略 `service` 时匹配任意服务下指定名称的方法。

## 条件组合（同一 match 内为 AND）

单个 `matches` 条目将其 `method` 和 `headers` 条件以 AND 关系组合——调用必须同时满足所有条件：

```yaml
spec:
  rules:
    - matches:
        - method:
            type: Exact
            service: user.UserService
            method: GetUser
          headers:
            - type: Exact
              name: x-api-version
              value: v2
      backendRefs:
        - name: user-service-v2
          port: 9000
```

## 多个 match（条目之间为 OR）

列出多个 `matches` 条目可将一组 RPC 路由到同一后端；匹配**任意**条目的调用均被接受：

```yaml
spec:
  rules:
    - matches:
        - method:
            type: Exact
            service: user.UserService
            method: GetUser
        - method:
            type: Exact
            service: user.UserService
            method: ListUsers
      backendRefs:
        - name: user-service
          port: 9000
```

## 正则表达式匹配

按方法名模式路由某服务的所有 RPC：

```yaml
spec:
  rules:
    - matches:
        - method:
            type: RegularExpression
            service: user.UserService
            method: "Get.*"
      backendRefs:
        - name: user-readonly-service
          port: 9000
```

## 优先级

当一个调用匹配多条规则时，最具体的规则获胜：`Exact` 方法匹配优于 `RegularExpression` 匹配，带有更多 headers 条件的匹配优于条件更少的匹配。应将较窄的规则放在宽泛的兜底规则之前。

## 相关文档

- [GRPCRoute 总览](../overview.md)
- [GRPCRoute 过滤器](../filters/overview.md)
- [GRPCRoute 后端配置](../backends/overview.md)
