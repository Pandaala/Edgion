# gRPC Listener

This document explains how to carry gRPC traffic over HTTP/HTTPS listeners and use GRPCRoute for routing.

## Key Points

- In the Gateway API, gRPC traffic typically runs on `HTTP` or `HTTPS` listeners.
- Edgion supports gRPC through HTTP/2 (h2/h2c).
- The routing layer uses `GRPCRoute` for matching.

## HTTPS + gRPC Example

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

## Notes

1. gRPC strongly depends on HTTP/2. Ensure that upstreams and clients negotiate h2.
2. For plaintext gRPC (h2c), use an HTTP listener and ensure proper network isolation at the boundary.
3. It is recommended to use access logs and metrics to observe gRPC error codes and latency.

## Related Documentation

- [HTTP Listener](./http.md)
- [HTTPS Listener](./https.md)
- [GRPCRoute Overview](../../../user-guide/grpc-route/overview.md)
