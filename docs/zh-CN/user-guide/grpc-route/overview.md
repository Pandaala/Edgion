# GRPCRoute 总览

GRPCRoute 用于基于 gRPC 服务名、方法名、主机名等条件进行路由，适用于微服务内部 gRPC 流量治理。

## 何时使用

- 服务间通信以 gRPC 为主。
- 需要按 service/method 做细粒度路由。
- 需要复用 Gateway 的流量治理能力。

## 最小示例

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: edge-gw
  namespace: gateway-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: grpc-https
      protocol: HTTPS
      port: 443
      hostname: grpc.example.com
      tls:
        mode: Terminate
        certificateRefs:
          - group: ""
            kind: Secret
            name: grpc-cert
---
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: account-route
  namespace: gateway-system
spec:
  parentRefs:
    - name: edge-gw
      sectionName: grpc-https
  hostnames:
    - grpc.example.com
  rules:
    - backendRefs:
        - name: account-service
          port: 50051
```

## 相关文档

- [匹配规则](./matches/overview.md)
- [过滤器](./filters/overview.md)
- [后端配置](./backends/overview.md)
