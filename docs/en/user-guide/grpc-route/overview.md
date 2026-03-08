# GRPCRoute Overview

GRPCRoute is used for routing based on gRPC service name, method name, hostname, and other conditions. It is suitable for managing gRPC traffic between microservices.

## When to Use

- Service-to-service communication primarily uses gRPC.
- Fine-grained routing by service/method is needed.
- You want to leverage the Gateway's traffic management capabilities.

## Minimal Example

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

## Related Documentation

- [Match Rules](./matches/overview.md)
- [Filters](./filters/overview.md)
- [Backend Configuration](./backends/overview.md)
