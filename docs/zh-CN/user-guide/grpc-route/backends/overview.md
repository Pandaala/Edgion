# GRPCRoute 后端配置

GRPCRoute 将 gRPC 调用转发到一个或多个通过 HTTP/2 提供 gRPC 服务的 Kubernetes Service。对 Service 端点的负载均衡是自动完成的；当列出多个后端时，按权重分配流量。

## 基本示例

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
    - backendRefs:
        - name: grpc-service
          port: 9000
```

## 后端字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | String | **必填** | 目标 Kubernetes Service 名称（必须通过 HTTP/2 提供 gRPC 服务） |
| `port` | Integer | **必填** | 连接到的 Service 端口 |
| `weight` | Integer | `1` | 发往此后端的流量相对份额 |
| `namespace` | String | GRPCRoute 所在命名空间 | Service 命名空间；跨命名空间需要 `ReferenceGrant` |
| `kind` | String | `Service` | 后端对象类型 |

## 带权重的后端

列出多个 `backendRefs` 可按权重（比例，非百分比）在后端之间分配调用——适用于服务新版本的金丝雀发布：

```yaml
spec:
  rules:
    - backendRefs:
        - name: user-service-stable
          port: 9000
          weight: 9
        - name: user-service-canary
          port: 9000
          weight: 1
```

`weight: 0` 的后端不接收新调用。

## 跨命名空间后端

若要转发到其他命名空间的 Service，在 `backendRef` 上设置 `namespace`，并在目标命名空间中创建允许 `GRPCRoute` 引用 `Service` 的 `ReferenceGrant`。

## 相关文档

- [GRPCRoute 总览](../overview.md)
- [GRPCRoute 匹配规则](../matches/overview.md)
- [跨命名空间引用](../../../ops-guide/infrastructure/reference-grant.md)
