# UDPRoute Overview

UDPRoute is used for UDP traffic forwarding, suitable for scenarios such as DNS, log collection, and game communication.

## Minimal Example

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

## Notes

1. UDP is connectionless; retry and session semantics differ from TCP/HTTP.
2. Reliability compensation should be handled at the backend application layer.

## Related Documentation

- [UDPRoute Backend Configuration](./backends/overview.md)
- [TCPRoute Stream Plugins](../tcp-route/stream-plugins.md)
