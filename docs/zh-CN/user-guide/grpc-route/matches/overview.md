# GRPCRoute 匹配规则

GRPCRoute 支持按主机名、gRPC 方法等维度匹配请求。

## 常见匹配维度

- Hostname：对应 TLS SNI/Host。
- Method：按 `service`/`method` 精准匹配。
- Header（如实现支持）：按元数据做扩展匹配。

## 示例：按服务和方法匹配

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: billing-route
  namespace: gateway-system
spec:
  parentRefs:
    - name: edge-gw
  rules:
    - matches:
        - method:
            service: billing.v1.BillingService
            method: CreateInvoice
      backendRefs:
        - name: billing-v1
          port: 50051
```

## 实践建议

1. 优先使用 service + method 精确匹配，减少歧义。
2. 对高优先级规则放在前面，避免被通配规则吞掉。
