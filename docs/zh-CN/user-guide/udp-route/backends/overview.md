# UDPRoute 后端配置

UDPRoute 将 UDP 数据报转发到一个或多个 Kubernetes Service。来自同一客户端的数据报在会话期间固定到同一后端；对端点的负载均衡是自动完成的。

## 基本示例

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: UDPRoute
metadata:
  name: dns-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
      sectionName: dns
  rules:
    - backendRefs:
        - name: dns-service
          port: 53
```

## 后端字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | String | **必填** | 目标 Kubernetes Service 名称 |
| `port` | Integer | **必填** | 发送数据报的 Service 端口 |
| `weight` | Integer | `1` | 发往此后端的会话相对份额 |
| `namespace` | String | UDPRoute 所在命名空间 | Service 命名空间；跨命名空间需要 `ReferenceGrant`。没有匹配的授权时，Gateway 在查找或向 Service 发送数据报之前即关闭会话。 |
| `kind` | String | `Service` | 后端对象类型 |

## 会话与超时

UDP 是无连接协议，因此网关按客户端源地址跟踪一个逻辑会话，并将其固定到某个后端。空闲会话在不活跃超时（60 秒）后被回收，此后来自该客户端的下一个数据报可能被负载均衡到不同的后端。

## 带权重的后端

列出多个 `backendRefs` 可按权重（比例，非百分比）在后端之间分配会话：

```yaml
spec:
  rules:
    - backendRefs:
        - name: dns-primary
          port: 53
          weight: 2
        - name: dns-secondary
          port: 53
          weight: 1
```

被拒绝的跨命名空间 backendRef 以及非 Service 的 `kind` 仍保留其声明权重。这些选中份额会失败关闭（`RefDenied` / `UnsupportedBackend`），而不会把会话重新分配给其余 Service。

## 相关文档

- [UDPRoute 总览](../overview.md)
- [跨命名空间引用](../../../ops-guide/infrastructure/reference-grant.md)
