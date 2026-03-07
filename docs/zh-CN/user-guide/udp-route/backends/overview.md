# UDPRoute 后端配置

UDPRoute 通过 `backendRefs` 将流量转发到目标 Service。

## 示例

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: UDPRoute
metadata:
  name: game-route
  namespace: gateway-system
spec:
  parentRefs:
    - name: edge-gw
      sectionName: udp-game
  rules:
    - backendRefs:
        - name: game-udp
          port: 30000
```

## 实践建议

1. 尽量保证后端实例稳定，避免频繁漂移。
2. 对延迟敏感业务，关注网络路径和丢包率。
