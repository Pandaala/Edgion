# GRPCRoute 匹配规则

GRPCRoute 按 gRPC 服务名和方法名匹配入站调用，也可选地按 gRPC 元数据（headers）匹配。当**同一个 `matches` 条目内的所有条件**均满足时，规则触发；列出多个 `matches` 条目相当于 OR 关系。

## Hostnames

`spec.hostnames` 最多接受 16 个小写 DNS 名称，也支持 `grpc` 这样的短名称。`*.example.com` 匹配 `api.example.com` 和 `a.b.example.com`，但不匹配 `example.com`。IP 地址、格式错误的名称和单独的 `*` 均无效；任一项无效会拒绝整个列表。省略或使用空列表表示路由不限制 hostname，但仍需通过监听器校验。

声明的精确名称优先于匹配的通配符后缀，通配符按后缀从长到短排序，最后才是未限制 hostname 的路由。监听器交集只收窄可用范围，不提升路由 hostname 的优先级。方法或请求头条件不满足时，可以继续检查其他合格候选，但不能绕过更具体的监听器。证书选择和证书通配符校验使用独立规则，本次不改变它们。

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

例如，`method: {method: GetUser}` 同时匹配 `/user.UserService/GetUser`
和 `/admin.AdminService/GetUser`。省略 `type` 时默认为 `Exact`。
省略外层 method 匹配器表示不限制服务和方法，因此只匹配 header 的规则仍然有效。
显式 `method: {}` 和空的 Exact 字段无效。正则匹配中，只有另一字段非空时，
缺失或为空的字段才表示不限制。无效的方法名称或正则表达式会使整条 rule 无效，
但保留其他有效 rule；Controller 状态会报告 `PartiallyInvalid`，全部有效规则候选
均无效时则报告 `Accepted=False / UnsupportedValue`。
Exact service 名称的配置校验遵循 Gateway API CEL 字符集合，包括 Unicode 简单
大小写折叠的等价字符；运行时 Exact 匹配仍按原始字符串区分大小写。

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

匹配候选按以下顺序比较：

1. 实际匹配的域名具体程度：精确域名、从长到短的通配后缀、不限制域名。
2. 配置的 service 字符数，再比较 method 字符数，较长者优先；正则使用模式字符数。
3. header 条件数量，较多者优先。
4. 路由创建时间，较早者优先；再按 `namespace/name` 字母顺序。
5. 同一路由内最先匹配的 rule，再比较该 rule 内的 match 声明顺序。

`Exact` 不会独立于以上规则获得高于正则的优先级。候选必须满足全部条件；
header 或 listener 不匹配时，会继续尝试后续候选及更宽泛的域名。
路由声明中未匹配请求的更长域名不会提高其优先级。

## 相关文档

- [GRPCRoute 总览](../overview.md)
- [GRPCRoute 过滤器](../filters/overview.md)
- [GRPCRoute 后端配置](../backends/overview.md)
