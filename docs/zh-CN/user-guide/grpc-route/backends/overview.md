# GRPCRoute 后端配置

GRPCRoute 通过 `backendRefs` 指定 gRPC 后端服务。

## 最小示例

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: order-route
  namespace: gateway-system
spec:
  parentRefs:
    - name: edge-gw
  rules:
    - backendRefs:
        - name: order-service
          port: 50051
          weight: 100
```

## 建议

1. 灰度发布场景可使用 `weight` 分流。
2. 若需要后端 TLS，请结合 HTTPRoute 后端 TLS 文档中的思路统一管理证书。

## 相关文档

- [Service 引用（HTTPRoute）](../../http-route/backends/service-ref.md)
- [权重配置（HTTPRoute）](../../http-route/backends/weight.md)
