---
name: tlsroute-features
description: Full TLSRoute schema: SNI-based TLS routing.
---

# TLSRoute Resource

> Canonical API: `gateway.networking.k8s.io/v1` | Scope: Namespaced
> Gateway API v1.5.0 Standard resource

TLSRoute routes based on the SNI (Server Name Indication) in the TLS ClientHello, supporting TLS passthrough or termination + forwarding.

## API version contract

Use `gateway.networking.k8s.io/v1`. The
[atomic API-version upgrade policy](../../08-kubernetes/04-api-version-upgrade.md)
requires one canonical version across preflight, watching, CRUD, and status
writes, with noncanonical FileSystem/static input rejected before publication.
Edgion does not promise older-version compatibility or authorize fallback
watchers and conversion shims merely because an installed CRD serves them.

The committed Gateway API v1.5.0 bundles declare:

| Bundle under `config/crd/gateway-api/` | TLSRoute versions served | Storage version |
|-------------------------------------|--------------------------|-----------------|
| `gateway-api-standard-v1.5.0.yaml` | `v1` | `v1` |
| `gateway-api-experimental-v1.5.0.yaml` | `v1`, `v1alpha2`, `v1alpha3` | `v1` |

Upstream served versions describe the API Server's schema, not an Edgion
multi-version support contract. Historical guidance advertised v1alpha3
fallback and automatic normalization; that described legacy implementation,
not the required current policy.

### Unresolved implementation discrepancy

Source inspection for this documentation correction found the following
remaining violations of the canonical-version contract. They require a
separate coordinated code change; this page does not claim they are fixed:

- `edgion-resources/src/resources/tls_route.rs` defines canonical `TLSRoute`
  at `v1`, but still contains `TLSRouteAlpha3` and conversions in both directions.
- Controller `conf_mgr/conf_center/kubernetes/version_detection.rs` probes
  `v1` then `v1alpha3`, including preflight. `controller.rs` selects one of the
  two typed watchers; this is single-active fallback, not simultaneous watches.
- Kubernetes `storage.rs` independently probes the version for CRUD.
  `resource_controller.rs` derives the status-patch version from its generic
  resource type, while `handlers/tls_route.rs::TlsRouteAlpha3Handler` adapts
  processing and status through the legacy type.
- `conf_mgr/conf_center/common.rs::parse_resource_content` converts v1alpha3
  TLSRoute input to v1. Both FileSystem and etcd resource controllers call this
  parser. `validate_resource_api_version` currently enforces the canonical
  boundary for BackendTLSPolicy only; it does not establish TLSRoute rejection.

The existing FileSystem test `parse_tls_route_v1alpha3_into_canonical_v1` and
resource conversion tests encode that observed legacy behavior. Their existence
is not approval to retain it. This audit inspected source and parsed both
committed CRDs; it did not execute tests or validate live cluster behavior.
Re-evaluate this discrepancy when version detection, watchers, CRUD/status,
resource types, or static-ingress validation changes. Removing only the prose
or one fallback branch cannot establish the required atomic upgrade.
The coordinated implementation is tracked in
[Remove the TLSRoute v1alpha3 fallback](../../../tasks/todo/remove-tlsroute-v1alpha3-fallback.md).

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
