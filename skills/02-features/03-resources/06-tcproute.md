---
name: tcproute-features
description: Full TCPRoute schema: TCP layer-4 routing.
---

# TCPRoute Resource

> API: `gateway.networking.k8s.io/v1alpha2` | Scope: Namespaced
> Gateway API v1.4.0 Experimental resource

TCPRoute defines routing rules for raw TCP traffic. TCP routes do not perform any protocol parsing; they only forward by port.

## Full Schema

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: my-tcp-route
  namespace: default
  annotations:
    edgion.io/edgion-stream-plugins: "default/my-stream-plugins"  # StreamPlugins binding
    edgion.io/proxy-protocol: "1"                                  # Proxy Protocol version
    edgion.io/max-connect-retries: "3"                             # Max connection retries
spec:
  parentRefs:
    - name: my-gateway
      sectionName: tcp

  rules:
    - backendRefs:
        - name: tcp-service
          port: 9000
          weight: 100
```

## spec Fields

| Field | Type | Description |
|-------|------|-------------|
| `parentRefs` | `Vec<ParentReference>` | Attach to a TCP-type Listener of a Gateway |
| `rules` | `Vec<TCPRouteRule>` | TCP routing rules (typically a single one) |

### TCPRouteRule

| Field | Type | Description |
|-------|------|-------------|
| `backendRefs` | `Vec<TCPBackendRef>` | TCP backend references |

### TCPBackendRef

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | `String` | Yes | Service name |
| `port` | `i32?` | No | Target port |
| `weight` | `i32?` | No | Traffic weight |
| `namespace` | `String?` | No | Namespace |
| `kind` | `String?` | No | Default `Service` |
| `group` | `String?` | No | Default `""` |

## Edgion Extension Annotations

| Annotation | Type | Default | Description |
|------------|------|---------|-------------|
| `edgion.io/edgion-stream-plugins` | `ns/name` | — | Bind to an EdgionStreamPlugins resource |
| `edgion.io/proxy-protocol` | `"1"\|"2"` | — | Send Proxy Protocol upstream |
| `edgion.io/max-connect-retries` | `u32` | `1` | Max upstream connection retry count |

## Characteristics

- **No match conditions**: TCP routes forward by port only and do not parse the protocol payload.
- **Per-listener isolation**: each Listener (indexed by `ListenerKey`) routes independently.
- **StreamPlugins**: bind TCP-layer plugins (e.g., IP restriction) via annotations.
