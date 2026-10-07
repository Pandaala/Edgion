---
name: udproute-features
description: Full UDPRoute schema: UDP layer-4 routing.
---

# UDPRoute Resource

> API: `gateway.networking.k8s.io/v1` | Scope: Namespaced
> Gateway API v1.6.2 Standard resource

UDPRoute defines routing rules for UDP traffic.

## Full Schema

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: UDPRoute
metadata:
  name: my-udp-route
  namespace: default
  annotations:
    edgion.io/edgion-stream-plugins: "default/my-stream-plugins"
spec:
  parentRefs:
    - name: my-gateway
      sectionName: udp

  rules:
    - backendRefs:
        - name: dns-service
          port: 53
          weight: 100
```

## spec Fields

| Field | Type | Description |
|-------|------|-------------|
| `parentRefs` | `Vec<ParentReference>` | Attach to a UDP-type Listener |
| `rules` | `Vec<UDPRouteRule>` | UDP routing rules |

### UDPRouteRule

| Field | Type | Description |
|-------|------|-------------|
| `backendRefs` | `Vec<UDPBackendRef>` | Backend references |

### UDPBackendRef

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | `String` | Yes | Service name |
| `port` | `i32?` | Yes for Service | Declared Service port, `1..=65535`; absence remains representable for validation |
| `weight` | `i32?` | No | Traffic weight |
| `namespace` | `String?` | No | Namespace |
| `kind` | `String?` | No | Default `Service` |
| `group` | `String?` | No | Default `""` |

## Characteristics

- **No match conditions**: UDP routes forward by port only.
- **Per-listener isolation**: each Listener (indexed by `ListenerKey`) routes independently.
- **Pinned client sessions**: the first packet selects an upstream; later packets reuse that address until the session is removed or expires after 60 seconds of inactivity. Route changes affect newly created sessions.
- **StreamPlugins**: `edgion.io/edgion-stream-plugins` runs the route-stage
  stream check before a new session is created. Established sessions bypass it. UDP has no TLS plugin
  stage and does not use TCP keepalive, Proxy Protocol, or connect retries.
