# TCPRoute Overview

TCPRoute is used for Layer 4 TCP traffic forwarding without parsing HTTP protocol semantics.

## Use Cases

- TCP protocols such as Redis, MySQL, PostgreSQL, MQTT.
- Routing bindings at the listener level are needed.

## Minimal Example

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

## Related Documentation

- [Backend Configuration](./backends/overview.md)
- [Stream Plugins](./stream-plugins.md)
