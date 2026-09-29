---
name: tlsroute-features
description: Full TLSRoute schema: SNI-based TLS routing.
---

# TLSRoute Resource

> Canonical API: `gateway.networking.k8s.io/v1` | Scope: Namespaced
> Gateway API v1.6.2 Standard resource

TLSRoute routes based on the SNI (Server Name Indication) in the TLS ClientHello.
TLS passthrough is enabled. Explicit TLS/Terminate listeners are currently rejected
with `Accepted=False/UnsupportedValue`; termination forwarding remains deferred.

## API version contract

Use `gateway.networking.k8s.io/v1`. The
[atomic API-version upgrade policy](../../08-kubernetes/04-api-version-upgrade.md)
requires one canonical version across preflight, watching, CRUD, and status
writes, with noncanonical FileSystem/static input rejected before publication.
Edgion does not promise older-version compatibility or authorize fallback
watchers and conversion shims merely because an installed CRD serves them.

The committed Gateway API v1.6.2 bundles declare:

| Bundle under `config/crd/gateway-api/` | TLSRoute versions served | Storage version |
|-------------------------------------|--------------------------|-----------------|
| `gateway-api-standard-v1.6.2.yaml` | `v1` | `v1` |
| `gateway-api-experimental-v1.6.2.yaml` | `v1`, `v1alpha2`, `v1alpha3` | `v1` |

Upstream served versions describe the API Server's schema, not an Edgion
multi-version support contract. Historical guidance advertised v1alpha3
fallback and automatic normalization; that described legacy implementation,
not the required current policy.

### Canonical input enforcement

Controller preflight, its single TLSRoute watcher, CRUD, and status writes use
v1. FileSystem, etcd, and Admin inputs validate the raw apiVersion before
deserialization and reject missing, malformed, or alternate versions. There
is no TLSRoute legacy type, converter, or fallback watcher.

See the [canonical ingress guard](../../04-review/controller/regression-guards/tls-route-canonical-api-ingress.md)
for source and regression-test evidence. Upstream CRDs remain unmodified;
their additional served versions do not expand Edgion's direct-input contract.

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
| `hostnames` | `Vec<String>` | Required nonempty SNI hostname match list (wildcard suffixes supported) |
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
| `edgion.io/edgion-stream-plugins` | `name` or `ns/name` | — | Bind to the resource's `tlsRoutePlugins` stage only; see [stage contract](14-edgion-stream-plugins.md#two-stage-model) |
| `edgion.io/proxy-protocol` | `"v2"` | disabled | Send Proxy Protocol v2 upstream; other values are ignored |
| `edgion.io/max-connect-retries` | `u32` | `2` | Max upstream connection attempts; values below `1` are normalized to `1`, values above `5` are capped at `5` |
| `edgion.io/tcp-keepalive-time` | positive integer seconds | disabled | Enable upstream TCP keepalive and set idle time; clamped to 1–7200 |
| `edgion.io/tcp-keepalive-interval` | integer seconds | `75` | Probe interval when keepalive is enabled; clamped to 1–75 |
| `edgion.io/tcp-keepalive-probes` | integer | `9` | Failed probes before closing; clamped to 1–20 |

## Coordination with Gateway TLS

| Gateway Listener TLS Mode | TLSRoute Behavior |
|---------------------------|-------------------|
| `Passthrough` | TLS passthrough: the Gateway does not terminate TLS and routes to the backend by SNI |
| `Terminate` | Deferred: Controller rejects this listener mode with `UnsupportedValue` |

Current product boundaries in Edgion:

- Standard `Passthrough` is supported.
- Standard `Terminate` is not currently supported; `TLSRouteModeTerminate` is
  not declared.
- `TLSRouteModeMixed` is not supported.
- BackendTLSPolicy does not apply to TLSRoute (Passthrough forwards the original TLS, and Terminate goes through the stream layer; use HTTPRoute/GRPCRoute when backend TLS is required).

## Differences from TCPRoute

| Dimension | TCPRoute | TLSRoute |
|-----------|----------|----------|
| Match dimensions | None (port only) | SNI hostname |
| Protocol awareness | None | TLS ClientHello |
| Multiplexing | One route per port | Multiple hostnames per port |
