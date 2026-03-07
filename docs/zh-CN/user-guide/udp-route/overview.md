# UDPRoute 总览

UDPRoute 用于 UDP 流量转发，适合 DNS、日志采集、游戏通信等场景。

## 最小示例

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: UDPRoute
metadata:
  name: dns-route
  namespace: gateway-system
spec:
  parentRefs:
    - name: edge-gw
      sectionName: udp-dns
  rules:
    - backendRefs:
        - name: dns-service
          port: 53
```

## 注意事项

1. UDP 无连接，重试与会话语义与 TCP/HTTP 不同。
2. 需要结合后端应用层做可靠性补偿。

## 相关文档

- [UDPRoute 后端配置](./backends/overview.md)
- [TCPRoute Stream Plugins](../tcp-route/stream-plugins.md)
