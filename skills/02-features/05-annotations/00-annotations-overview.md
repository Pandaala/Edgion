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
| `edgion.io/edgion-plugins` | comma/space-separated refs | Ordered global HTTP plugins; bare names use the Gateway namespace |
| `edgion.io/require-sni-host-match` | `"true"\|"false"` | Per-Gateway override of `spec.securityProtect.requireSniHostMatch` |
| `edgion.io/dns-resolver` | comma/space-separated `ip[:port]` | Data-plane DNS servers; mutually exclusive with `dns-resolver-ref` |
| `edgion.io/dns-resolver-ref` | `namespace/name` | Data-plane httpdns LinkSys reference; mutually exclusive with `dns-resolver` |
| `edgion.io/metrics-test-key` | `string` | Metrics label key for testing |
| `edgion.io/metrics-test-type` | `string` | Metrics label value for testing |

### HTTPRoute / GRPCRoute

| Annotation | Value | Description |
|------------|-------|-------------|
| `edgion.io/hostname-resolution` | — | System-managed (do not set manually) |
| `edgion.io/log-label.<key>` | string | Adds `<key>=<value>` to `rsc.labels`; label/annotation projection also applies to TLSRoute and EdgionTls. |
| `edgion.io/log-ctx-keys` | comma-separated list | Opt-in allowlist of plugin-set ctx-var (`ctx_map`) keys to project into the access log. Context variables are **not** logged by default; only keys named here are emitted (known-sensitive keys like claims stay hard-blocked). Shorthand for `log-keys` entries of `type: ctx`. |
| `edgion.io/log-keys` | JSON array | Unified opt-in log-projection carrier. Each entry is `{"key": "<name>", "type": "ctx"\|"label"\|"annotation"}`. `type: ctx` adds `<name>` to the access-log ctx allowlist; `type: label` / `type: annotation` projects the value of the existing `metadata.labels[<name>]` / `metadata.annotations[<name>]` into `rsc.labels`. Malformed entries are skipped individually; merges with `log-label.<key>` and `log-ctx-keys`. |

Log projection is capped at 16 entries; keys at 63 bytes and values at 256 bytes.
Context projection applies only to HTTPRoute/GRPCRoute. Label/annotation projection also
applies to TLSRoute and EdgionTls.

- `edgion.io/forward-raw-path` (HTTPRoute) — three-state control over the upstream path forwarding view:
  - `"true"` forces raw, `"false"` forces normalized (overrides region_route); absent falls back to the default flow.
  - See the [metadata reference](references/annotations-metadata.md) for details.

- `edgion.io/retry-non-idempotent` (HTTPRoute / GRPCRoute) — opt-in to retrying non-idempotent
  methods on the reused-connection retry path. By default a reused keepalive connection that is
  reset mid-request is **not** retried for non-idempotent methods (POST/PATCH/CONNECT, and all
  gRPC since it is POST at the HTTP layer), because replaying the buffered request body risks a
  silent duplicate mutation. Idempotent methods (GET/HEAD/PUT/DELETE/OPTIONS/TRACE) are always
  retried. Set `"true"` to restore the pre-guard behavior for this route. The knob lives in an
  annotation because the standard Gateway API `HTTPRouteRetry` CRD is structural and prunes
  unknown fields. See the [metadata reference](references/annotations-metadata.md) for details.

### TCPRoute / TLSRoute / UDPRoute

| Annotation | Value | Description |
|------------|-------|-------------|
| `edgion.io/edgion-stream-plugins` | `ns/name` | StreamPlugins binding (TCPRoute / TLSRoute / UDPRoute) |
| `edgion.io/proxy-protocol` | `"v2"` | Send Proxy Protocol v2 to upstream (TLSRoute only; other values ignored) |
| `edgion.io/max-connect-retries` | `u32` | Max upstream connect attempts (TLSRoute only; clamped to 1..=5, default 2) |
| `edgion.io/tcp-keepalive-time` | positive integer string | Idle seconds before the first keepalive probe on the upstream L4 connection (clamped 1–7200). Presence enables keepalive; absent = disabled (TCPRoute / TLSRoute) |
| `edgion.io/tcp-keepalive-interval` | integer string | Seconds between keepalive probes; clamped 1–75; absent or invalid = 75. Only applied when `tcp-keepalive-time` is set (TCPRoute / TLSRoute) |
| `edgion.io/tcp-keepalive-probes` | integer string | Failed probe count before connection is declared dead; clamped 1–20; absent or invalid = 9. Only applied when `tcp-keepalive-time` is set (TCPRoute / TLSRoute) |

- On `UDPRoute`, `edgion.io/edgion-stream-plugins` runs only the Stage-1 ConnectionFilter (`spec.plugins`, IP-based, fail-close); there is no TLS stage.

### EdgionTls

| Annotation | Value | Description |
|------------|-------|-------------|
| `edgion.io/expose-client-cert` | `"true"\|"false"` | Expose parsed client-certificate information to plugin/session/DSL/auth consumers; does not inject headers automatically |

### Service / EndpointSlice

| Annotation | Value | Description |
|------------|-------|-------------|
| `edgion.io/dns-resolution-mode` | `controller\|gateway\|per-request` | ExternalName resolution authority/cache mode; default `controller` |
| `edgion.io/dns-resolver` | comma/space-separated `ip[:port]` | Service resolver override for both planes |
| `edgion.io/dns-resolver-ref` | `namespace/name` | Service httpdns LinkSys override for both planes |

Active health checks, outlier detection (passive ejection / circuit breaking), and
the per-Service panic-threshold override are **not** annotations — they are
configured via the `EdgionBackendTrafficPolicy` CRD (`spec.healthCheck`,
`spec.outlierDetection`, `spec.loadBalancer.panicThreshold`), attached to a
Service via `targetRefs`. See
[03-resources/20-edgion-backend-traffic-policy.md](../03-resources/20-edgion-backend-traffic-policy.md)
for the full schema, defaults, and conflict semantics.

> **Backend protocol selection**: not controlled via annotation; use the standard Service `spec.ports[].appProtocol` field (GEP-1911). `kubernetes.io/h2c` (HTTP/2 cleartext upstream) and `kubernetes.io/ws` (WebSocket) are supported. See [annotations-metadata.md](references/annotations-metadata.md) for details.

## Options Keys

### Gateway Listener `tls.options`

| Key | Value | Description |
|-----|-------|-------------|
| `edgion.io/cert-provider` | exact `"EdgionTls"` | Enable the dynamic EdgionTls provider; absent uses normal certificateRefs/Secret behavior |

### BackendTLSPolicy `spec.options`

| Key | Value | Description |
|-----|-------|-------------|
| `edgion.io/client-certificate-ref` | same-namespace Secret name | Upstream mTLS client certificate; slash/cross-namespace values are invalid |

## Labels

| Label | Value | Description |
|-------|-------|-------------|
| `edgion.io/leader` | `"true"\|"false"` | Pod leader-election marker; the leader Service selects `"true"` |
| `edgion.io/managed-by` | see below | Lifecycle ownership of the labeled resource |
| `edgion.io/acme-resource` | resource name | ACME resource association |

### `edgion.io/managed-by` Vocabulary

Applies to `EdgionConfigData` (and `Secret`/`EdgionTls` managed by ACME). Declares who owns the resource
lifecycle and how the controller treats the spec on reconcile.

| Value | Active? | Description |
|-------|---------|-------------|
| `operator` | yes (default) | Git/kubectl authoritative; controller never mutates spec. This is the only active mode today. |
| `acme` | yes | ACME service manages the certificate lifecycle (existing usage on Secrets and EdgionTls). |
| `center` | **DESIGN-DECLARED, NOT YET IMPLEMENTED** | Placeholder; real data delivered via fed_sync from EdgionCenter. Controller does not overwrite spec from git. Refs always resolve (object exists); overlay-absent fallback yields base. Requires EdgionCenter coordination. |
| `controller` | **DESIGN-DECLARED, NOT YET IMPLEMENTED** | Controller regenerates body from a template ref. Requires template-engine design. |

Constant definitions: `edgion-resources/src/constants/labels.rs` (`managed_by` module).

## System Reserved Keys

The following annotations are managed automatically by the system; **do not set them manually**:

| Annotation | Description |
|------------|-------------|
| `edgion.io/hostname-resolution` | DNS resolution results injected by the Controller |
| `edgion.io/sync-version` | Configuration sync version number |
| `edgion.io/dns-resolved-at` | Controller-written freshness timestamp on synthetic EndpointSlices |

See the [references/](references/) directory for the detailed reference.
