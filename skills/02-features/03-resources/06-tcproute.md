---
name: tcproute-features
description: Full TCPRoute schema: TCP layer-4 routing.
---

# TCPRoute Resource

> API: `gateway.networking.k8s.io/v1` | Scope: Namespaced
> Gateway API v1.6.2 Standard resource

TCPRoute defines routing rules for raw TCP traffic. TCP routes do not perform any protocol parsing; they only forward by port.

## Full Schema

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: TCPRoute
metadata:
  name: my-tcp-route
  namespace: default
  annotations:
    edgion.io/edgion-stream-plugins: "default/my-stream-plugins"  # StreamPlugins binding
    edgion.io/tcp-keepalive-time: "300"                            # Enable upstream keepalive
    edgion.io/tcp-keepalive-interval: "30"
    edgion.io/tcp-keepalive-probes: "5"
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
| `port` | `i32?` | Yes for Service | Declared Service port, `1..=65535`; absence remains representable for validation |
| `weight` | `i32?` | No | Traffic weight |
| `namespace` | `String?` | No | Namespace |
| `kind` | `String?` | No | Default `Service` |
| `group` | `String?` | No | Default `""` |

## Edgion Extension Annotations

| Annotation | Type | Default | Description |
|------------|------|---------|-------------|
| `edgion.io/edgion-stream-plugins` | `ns/name` | — | Bind to an EdgionStreamPlugins resource |
| `edgion.io/tcp-keepalive-time` | positive integer seconds | disabled | Enable upstream TCP keepalive and set idle time; clamped to 1–7200 |
| `edgion.io/tcp-keepalive-interval` | integer seconds | `75` | Probe interval when keepalive is enabled; clamped to 1–75 |
| `edgion.io/tcp-keepalive-probes` | integer | `9` | Failed probes before closing; clamped to 1–20 |

`edgion.io/proxy-protocol` and `edgion.io/max-connect-retries` are TLSRoute-only runtime annotations; TCPRoute does not read them.

## Characteristics

- **No match conditions**: TCP routes forward by port only and do not parse the protocol payload.
- **Per-listener isolation**: each Listener (indexed by `ListenerKey`) routes independently.
- **StreamPlugins**: bind TCP-layer plugins (e.g., IP restriction) via annotations.
