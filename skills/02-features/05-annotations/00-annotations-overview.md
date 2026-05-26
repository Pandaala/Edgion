---
name: annotations-overview
description: Edgion annotations overview: a quick lookup for all edgion.io/* annotations grouped by resource type.
---

# Annotations Overview

## Prefix Convention

- All Edgion annotations use the `edgion.io/` prefix (**not** `edgion.com/`).
- The Stream plugin annotation key is `edgion-stream-plugins` (**not** `stream-plugins`).

## Lookup by Resource Type

### Gateway

| Annotation | Value | Description |
|------------|-------|-------------|
| `edgion.io/enable-http2` | `"true"\|"false"` | Listener-side HTTP/2 (default true): HTTP→h2c, HTTPS→ALPN h2 |
| `edgion.io/http-to-https-redirect` | `"true"\|"false"` | HTTP→HTTPS redirect |
| `edgion.io/https-redirect-port` | `port` | HTTPS redirect port |
| `edgion.io/edgion-stream-plugins` | `ns/name` | StreamPlugins binding |
| `edgion.io/require-sni-host-match` | `"true"` | Enable HTTPS listener 421 Misdirected Request detection (per-Gateway opt-in when globally disabled) |
| `edgion.io/metrics-test-key` | `string` | Metrics label key for testing |
| `edgion.io/metrics-test-type` | `string` | Metrics label value for testing |

### HTTPRoute / GRPCRoute

| Annotation | Value | Description |
|------------|-------|-------------|
| `edgion.io/hostname-resolution` | — | System-managed (do not set manually) |

- `edgion.io/forward-raw-path` (HTTPRoute) — three-state control over the upstream path forwarding view:
  - `"true"` forces raw, `"false"` forces normalized (overrides region_route); absent falls back to the default flow.
  - See spec §14.6 / metadata reference for details.

### TCPRoute / TLSRoute / UDPRoute

| Annotation | Value | Description |
|------------|-------|-------------|
| `edgion.io/edgion-stream-plugins` | `ns/name` | StreamPlugins binding (TCPRoute / TLSRoute / UDPRoute) |
| `edgion.io/proxy-protocol` | `"1"\|"2"` | Proxy Protocol version (TCPRoute / TLSRoute) |
| `edgion.io/max-connect-retries` | `u32` | Max connection retries (TCPRoute / TLSRoute) |

- On `UDPRoute`, `edgion.io/edgion-stream-plugins` runs only the Stage-1 ConnectionFilter (`spec.plugins`, IP-based, fail-close); there is no TLS stage.

### EdgionTls

| Annotation | Value | Description |
|------------|-------|-------------|
| `edgion.io/expose-client-cert` | `"true"\|"false"` | Expose client certificate to request headers |

### Service / EndpointSlice / Endpoints

| Annotation | Value | Description |
|------------|-------|-------------|
| `edgion.io/health-check` | YAML string | Health check configuration |
| `edgion.io/circuit-breaker` | YAML string | Circuit breaker configuration (consecutiveErrors/ejectionSeconds/maxEjectionPercent) |

> **Backend protocol selection**: not controlled via annotation; use the standard Service `spec.ports[].appProtocol` field (GEP-1911). `kubernetes.io/h2c` (HTTP/2 cleartext upstream) and `kubernetes.io/ws` (WebSocket) are supported. See [annotations-metadata.md](references/annotations-metadata.md) for details.

## Options Keys

### Gateway Listener `tls.options`

| Key | Value | Description |
|-----|-------|-------------|
| `edgion.io/cert-provider` | `"secret"\|"edgion-tls"` | Certificate source |

### BackendTLSPolicy `spec.options`

| Key | Value | Description |
|-----|-------|-------------|
| `edgion.io/client-certificate-ref` | `ns/name` | Client certificate Secret |

## Labels

| Label | Value | Description |
|-------|-------|-------------|
| `edgion.io/leader` | pod name | Leader election marker |
| `edgion.io/managed-by` | `"acme"` | ACME management marker |
| `edgion.io/acme-resource` | resource name | ACME resource association |

## System Reserved Keys

The following annotations are managed automatically by the system; **do not set them manually**:

| Annotation | Description |
|------------|-------------|
| `edgion.io/hostname-resolution` | DNS resolution results injected by the Controller |
| `edgion.io/sync-version` | Configuration sync version number |

See the [references/](references/) directory for the detailed reference.
