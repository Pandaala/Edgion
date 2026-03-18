# 第一个 Gateway

本页给你一个最小可理解的对象组合：

1. `GatewayClass`
2. `Gateway`
3. `HTTPRoute`

目标不是覆盖所有细节，而是先让你知道一条流量是怎么从入口走到后端的。

## Step 1: GatewayClass

`GatewayClass` 决定由哪个控制器来管理 Gateway。

最小示例：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: public-gateway
spec:
  controllerName: edgion.io/gateway-controller
```

如果你需要 Edgion 的网关级高级配置，可以继续给它加 `parametersRef`，见：

- [运维指南 / GatewayClass 配置](../ops-guide/gateway/gateway-class.md)

## Step 2: Gateway

`Gateway` 定义流量入口，也就是 listener。

最小 HTTP 示例：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: internal-gateway
  namespace: edgion-test
spec:
  gatewayClassName: public-gateway
  listeners:
    - name: http
      protocol: HTTP
      port: 80
```

这一步确定了：

- 用哪个 `GatewayClass`
- 暴露哪些协议和端口
- route 之后要绑定到哪个 listener

更多 listener 细节见：

- [运维指南 / Gateway 总览](../ops-guide/gateway/overview.md)
- [运维指南 / HTTP 监听器](../ops-guide/gateway/listeners/http.md)

## Step 3: HTTPRoute

`HTTPRoute` 把请求绑定到 Gateway listener，并把流量转发给后端服务。

最小示例：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: example-route
  namespace: edgion-test
spec:
  parentRefs:
    - name: internal-gateway
      sectionName: http
  hostnames:
    - "example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /
      backendRefs:
        - name: echo-service
          port: 8080
```

这里最关键的是：

- `parentRefs` 把 route 绑定到 `Gateway`
- `sectionName` 对应 listener 名称
- `backendRefs` 指向真正接收流量的服务

更多细节见：

- [用户指南 / HTTPRoute 总览](../user-guide/http-route/overview.md)
- [用户指南 / Service 引用](../user-guide/http-route/backends/service-ref.md)

## Step 4: 理解这条流量

这三个对象串起来以后，请求路径大致是：

1. 客户端请求打到 `Gateway` 的 listener
2. `HTTPRoute` 按 hostname / path / headers 等规则匹配
3. 选中 `backendRefs`
4. Gateway 把请求转发到后端服务

如果后面要继续加能力，通常是在 `HTTPRoute` 上继续叠加：

- 匹配规则
- 标准 Gateway API filters
- Edgion 扩展插件
- 重试、超时、会话保持

## 下一步建议

如果你只是要先会用，继续读：

- [用户指南 / HTTPRoute 总览](../user-guide/http-route/overview.md)
- [用户指南 / 过滤器总览](../user-guide/http-route/filters/overview.md)
- [用户指南 / 后端配置](../user-guide/http-route/backends/README.md)

如果你还没完全理解对象关系，先回去读：

- [核心概念](./core-concepts.md)
