# TCP Listener

This document explains how to configure a TCP listener on a Gateway and bind TCPRoutes to the corresponding port.

## Use Cases

- Proxying Layer 4 TCP protocols such as MySQL, Redis, MQTT, etc.
- Attaching multiple TCPRoutes to the same Gateway using `sectionName`.

## Basic Configuration

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: edge-gw
  namespace: gateway-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: tcp-redis
      protocol: TCP
      port: 6379
      allowedRoutes:
        namespaces:
          from: Same
```

## Binding with TCPRoute

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

## Notes

1. `listener.protocol` must be `TCP`.
2. `TCPRoute.parentRefs.sectionName` must match the listener `name`.
3. Cross-namespace binding requires a `ReferenceGrant`.

## Related Documentation

- [Gateway Resource Overview](../overview.md)
- [TCPRoute and Stream Plugins](../../../user-guide/tcp-route/stream-plugins.md)
- [Cross-Namespace Reference](../../infrastructure/reference-grant.md)
