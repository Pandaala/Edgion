# TCP 监听器

本文档说明如何在 Gateway 上配置 TCP 监听器并让 TCPRoute 绑定到对应端口。

## 适用场景

- 代理 MySQL、Redis、MQTT 等四层 TCP 协议。
- 需要按 `sectionName` 将多个 TCPRoute 挂到同一个 Gateway。

## 基础配置

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: edge-gw
  namespace: gateway-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: tcp-redis
      protocol: TCP
      port: 6379
      allowedRoutes:
        namespaces:
          from: Same
```

## 与 TCPRoute 绑定

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: redis-route
  namespace: gateway-system
spec:
  parentRefs:
    - name: edge-gw
      sectionName: tcp-redis
  rules:
    - backendRefs:
        - name: redis
          port: 6379
```

## 注意事项

1. `listener.protocol` 必须为 `TCP`。
2. `TCPRoute.parentRefs.sectionName` 必须和监听器 `name` 一致。
3. 跨命名空间绑定时，需要配合 `ReferenceGrant`。

## 相关文档

- [Gateway 资源总览](../overview.md)
- [TCPRoute 与 Stream Plugins](../../../user-guide/tcp-route/stream-plugins.md)
- [跨命名空间引用](../../infrastructure/reference-grant.md)
