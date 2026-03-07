# gRPC 监听器

本文档说明如何通过 HTTP/HTTPS 监听器承载 gRPC 流量，并配合 GRPCRoute 做路由。

## 关键点

- Gateway API 中 gRPC 流量通常运行在 `HTTP` 或 `HTTPS` 监听器上。
- Edgion 通过 HTTP/2（h2/h2c）支持 gRPC。
- 路由层使用 `GRPCRoute` 进行匹配。

## HTTPS + gRPC 示例

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
```

## 注意事项

1. gRPC 强依赖 HTTP/2，请确保上游和客户端协商为 h2。
2. 如果走明文 gRPC（h2c），请使用 HTTP 监听器并在网络边界做好隔离。
3. 建议配合访问日志与指标观测 gRPC 错误码和时延。

## 相关文档

- [HTTP 监听器](./http.md)
- [HTTPS 监听器](./https.md)
- [GRPCRoute 总览](../../../user-guide/grpc-route/overview.md)
