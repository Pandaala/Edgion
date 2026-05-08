---
name: udproute-features
description: Full UDPRoute schema: UDP layer-4 routing.
---

# UDPRoute Resource

> API: `gateway.networking.k8s.io/v1alpha2` | Scope: Namespaced
> Gateway API v1.4.0 Experimental resource

UDPRoute defines routing rules for UDP traffic.

## Full Schema

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: UDPRoute
metadata:
  name: my-udp-route
  namespace: default
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
| `port` | `i32?` | No | Target port |
| `weight` | `i32?` | No | Traffic weight |
| `namespace` | `String?` | No | Namespace |
| `kind` | `String?` | No | Default `Service` |
| `group` | `String?` | No | Default `""` |

## Characteristics

- **No match conditions**: UDP routes forward by port only.
- **Per-listener isolation**: each Listener (indexed by `ListenerKey`) routes independently.
- **No connection state**: UDP is connectionless; each datagram is routed independently.
