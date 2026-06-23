# Service 引用

`backendRef` 将 HTTPRoute 规则指向 Kubernetes Service。Gateway（数据面）将该 Service 解析为其活跃端点，并在这些端点之间做负载均衡。

## 基本配置

```yaml
backendRefs:
  - name: api-service
    port: 8080
```

## 配置参考

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | String | **必填** | Service 名称 |
| `port` | Integer | **必填** | Service 端口（Service 暴露的 `port`，非 Pod 的 `targetPort`） |
| `namespace` | String | 路由所在命名空间 | Service 命名空间；跨命名空间需要 ReferenceGrant |
| `kind` | String | `Service` | 后端类型。允许值：`Service` \| `ServiceClusterIp` \| `ServiceExternalName` |
| `group` | String | `""` | API 组；核心 Service 使用 `""` |
| `weight` | Integer | `1` | 相对流量权重，详见 [权重配置](./weight.md) |

## backendRef 如何解析为端点

1. `name`（+ `namespace`）标识对应的 Service。
2. `port` 选择匹配的 Service 端口。Edgion 将其映射到 Pod 侧的 `targetPort` 和对应的 EndpointSlice。
3. Controller（控制面）监视 Service 的 EndpointSlice，并通过 gRPC 将就绪的端点地址推送给 Gateway。
4. Gateway 在这些端点之间对请求进行负载均衡，跳过已被健康检查或熔断器标记为不健康的端点。

端点成员关系来自 EndpointSlice API（而非旧式的单一 `Endpoints` 对象），因此端点集可增量扩展和更新。当 Pod 被添加、移除或就绪状态变化时，更新后的 EndpointSlice 会自动流转到 Gateway，无需重新应用路由。

Service 发生变更（例如端口重命名）时，所有引用该 Service 的路由都会自动触发重处理，保持依赖路由的一致性。

## 默认行为

### kind 字段

- 未指定 `kind` 时，默认为 `Service`。
- `ServiceExternalName` 会解析 Service 的 `externalName`（DNS）目标，而非集群内部的 EndpointSlice。

### namespace 字段

- 省略 `namespace` 时，Gateway 使用 HTTPRoute 自身所在的命名空间。
- 引用不同命名空间的 Service 需要在**目标**命名空间中创建 ReferenceGrant。

### 无法解析的后端

如果引用的 Service 或端口无法解析（名称错误、缺少 ReferenceGrant，或没有就绪的端点），路由的 `ResolvedRefs` 状态会反映故障，对该后端的请求返回 5xx。修复引用后，一旦端点可用，Gateway 会自动恢复。

## 跨命名空间引用

引用其他命名空间的 Service 需要在 **Service 所在命名空间**中创建 ReferenceGrant。若没有该授权，即使 Service 存在，引用也会被拒绝：

```yaml
# production 命名空间中的 HTTPRoute
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - backendRefs:
        - name: backend-service
          namespace: staging
          port: 8080

---
# 在目标命名空间（staging）中创建的 ReferenceGrant
apiVersion: gateway.networking.k8s.io/v1
kind: ReferenceGrant
metadata:
  name: allow-from-production
  namespace: staging
spec:
  from:
    - group: gateway.networking.k8s.io
      kind: HTTPRoute
      namespace: production
  to:
    - group: ""
      kind: Service
```

## 相关文档

- [权重配置](./weight.md)
- [后端 TLS](./backend-tls.md)
- [后端主动健康检查](./health-check.md)
- [熔断器](./circuit-breaker.md)
