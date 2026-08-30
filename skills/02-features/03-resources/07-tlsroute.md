---
name: tlsroute-features
description: Full TLSRoute schema: SNI-based TLS routing.
---

# TLSRoute Resource

> API: `gateway.networking.k8s.io/v1` (v1alpha3 compatible) | Scope: Namespaced
> Gateway API v1.5.0 Standard resource (v1alpha3 in v1.4.0 Experimental)

TLSRoute routes based on the SNI (Server Name Indication) in the TLS ClientHello, supporting TLS passthrough or termination + forwarding.

### Multi-version Compatibility

| Install Manifest | v1alpha3 | v1 |
|------------------|----------|-----|
| v1.4.0 experimental | **served=true, storage=true** | Does not exist |
| v1.5.0 standard | served=false | **served=true, storage=true** |

The Edgion controller prefers to watch v1, falling back to v1alpha3 (single-active watch) when v1 is unavailable. v1alpha3 → v1 is a pure version promotion, with identical spec field structure. In FileSystem mode, v1alpha3 YAML is automatically normalized to v1 at the parsing entry.

## Full Schema

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: TLSRoute
metadata:
  name: my-tls-route
  namespace: default
  annotations:
    edgion.io/edgion-stream-plugins: "default/my-stream-plugins"
    edgion.io/proxy-protocol: "v2"
    edgion.io/max-connect-retries: "3"             # Optional; default 2, max 5
    edgion.io/tcp-keepalive-time: "300"
    edgion.io/tcp-keepalive-interval: "30"
    edgion.io/tcp-keepalive-probes: "5"
spec:
  parentRefs:
    - name: my-gateway
      sectionName: tls-passthrough

  hostnames:                                       # SNI matching
    - "secure.example.com"
    - "*.internal.example.com"

  rules:
    - backendRefs:
        - name: tls-backend
          port: 8443
          weight: 100
```

## spec Fields

| Field | Type | Description |
|-------|------|-------------|
| `parentRefs` | `Vec<ParentReference>` | Attach to a TLS-type Listener |
| `hostnames` | `Vec<String>?` | SNI hostname match list (wildcards supported) |
| `rules` | `Vec<TLSRouteRule>` | TLS routing rules |

### TLSRouteRule

| Field | Type | Description |
|-------|------|-------------|
| `backendRefs` | `Vec<TLSBackendRef>` | Backend references |

### TLSBackendRef

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
| `edgion.io/proxy-protocol` | `"v2"` | disabled | Send Proxy Protocol v2 upstream; other values are ignored |
| `edgion.io/max-connect-retries` | `u32` | `2` | Max upstream connection attempts; values below `1` are normalized to `1`, values above `5` are capped at `5` |
| `edgion.io/tcp-keepalive-time` | positive integer seconds | disabled | Enable upstream TCP keepalive and set idle time; clamped to 1–7200 |
| `edgion.io/tcp-keepalive-interval` | integer seconds | `75` | Probe interval when keepalive is enabled; clamped to 1–75 |
| `edgion.io/tcp-keepalive-probes` | integer | `9` | Failed probes before closing; clamped to 1–20 |

## Coordination with Gateway TLS

| Gateway Listener TLS Mode | TLSRoute Behavior |
|---------------------------|-------------------|
| `Passthrough` | TLS passthrough: the Gateway does not terminate TLS and routes to the backend by SNI |
| `Terminate` | TLS termination: after decryption, the Gateway routes to the backend by SNI |

Current product boundaries in Edgion:

- Standard `Passthrough` is supported.
- Standard `Terminate` is supported.
- `TLSRouteModeMixed` is not supported.
- BackendTLSPolicy does not apply to TLSRoute (Passthrough forwards the original TLS, and Terminate goes through the stream layer; use HTTPRoute/GRPCRoute when backend TLS is required).

## Differences from TCPRoute

| Dimension | TCPRoute | TLSRoute |
|-----------|----------|----------|
| Match dimensions | None (port only) | SNI hostname |
| Protocol awareness | None | TLS ClientHello |
| Multiplexing | One route per port | Multiple hostnames per port |
