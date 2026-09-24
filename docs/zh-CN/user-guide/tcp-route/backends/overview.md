# TCPRoute 后端配置

TCPRoute 将原始 TCP 流转发到一个或多个 Kubernetes Service。对 Service 端点的负载均衡是自动完成的；当列出多个后端时，按权重在它们之间分配连接。

## 基本示例

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: TCPRoute
metadata:
  name: database-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: postgres
  rules:
    - backendRefs:
        - name: database-service
          port: 5432
```

## 后端字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | String | **必填** | 目标 Kubernetes Service 名称 |
| `port` | Integer | **必填** | 连接到的 Service 端口 |
| `weight` | Integer | `1` | 发往此后端的连接相对份额 |
| `namespace` | String | TCPRoute 所在命名空间 | Service 命名空间；跨命名空间需要 `ReferenceGrant` |
| `kind` | String | `Service` | 后端对象类型 |

## 带权重的后端

列出多个 `backendRefs` 可按权重在后端之间分配新连接。权重是比例而非百分比——`weight: 3` 和 `weight: 1` 大约将连接分配为 75% / 25%：

```yaml
spec:
  rules:
    - backendRefs:
        - name: database-primary
          port: 5432
          weight: 3
        - name: database-canary
          port: 5432
          weight: 1
```

`weight: 0` 的后端不接收新连接，适用于在移除之前先将其排空。

被拒绝的跨命名空间 backendRef 以及非 Service 的 `kind` 仍保留其声明权重。这些选中份额会失败关闭（`RefDenied` / `UnsupportedBackend`），而不会把流量重新分配给其余 Service。

## 跨命名空间后端

若要转发到其他命名空间的 Service，在 `backendRef` 上设置 `namespace`，并在目标命名空间中创建允许 `TCPRoute` 引用 `Service` 的 `ReferenceGrant`。没有匹配的授权时，Controller 会将 backendRef 标记为拒绝，Gateway 在查找或拨号 Service 之前即关闭连接：

```yaml
apiVersion: gateway.networking.k8s.io/v1beta1
kind: ReferenceGrant
metadata:
  name: allow-tcp-to-db
  namespace: data
spec:
  from:
    - group: gateway.networking.k8s.io
      kind: TCPRoute
      namespace: production
  to:
    - group: ""
      kind: Service
```

## 相关文档

- [TCPRoute 总览](../overview.md)
- [Stream Plugins](../stream-plugins.md)
- [跨命名空间引用](../../../ops-guide/infrastructure/reference-grant.md)
