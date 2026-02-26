# TCPRoute 总览

TCPRoute 用于四层 TCP 流量转发，不解析 HTTP 协议语义。

## 适用场景

- Redis、MySQL、PostgreSQL、MQTT 等 TCP 协议。
- 需要按监听器维度做路由绑定。

## 最小示例

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

## 相关文档

- [后端配置](./backends/overview.md)
- [Stream Plugins](./stream-plugins.md)
