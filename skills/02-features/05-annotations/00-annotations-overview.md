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
| `edgion.io/edgion-stream-plugins` | `name` | Same-namespace StreamPlugins binding; slash-containing values are rejected |
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

To project route metadata, request/response fields, or plugin-set ctx vars
into the access log on HTTPRoute / GRPCRoute, use an `AccessLogExtern`
declaration (`EdgionPlugins.spec.accessLogExtern`, attached via an
`ExtensionRef` filter) — **not** an annotation. See
[`../../03-coding/observability/00-access-log.md`](../../03-coding/observability/00-access-log.md#extending-the-access-log-accesslogextern).
The old ctx-key allowlist annotation and the old unified JSON log-projection
annotation have both been removed entirely (no aliases, no migration shim);
`edgion.io/log-label.<key>` is **narrowed to
TLSRoute / EdgionTls only** — see the TCPRoute/TLSRoute/UDPRoute table below —
because the HTTP/gRPC path now has `AccessLogExtern` and the TLS handshake
layer has no plugin chain to attach a declaration to.

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

- `edgion.io/mirror-*` (HTTPRoute) — RequestMirror tuning knobs, applied to every
  `RequestMirror` filter under the route. Same rationale as `retry-non-idempotent`: the
  vendored Gateway API CRD prunes non-standard `requestMirror` fields in K8s mode, so
  tuning lives in annotations (the `HTTPRequestMirrorFilter` struct keeps only the
  standard `backendRef`/`fraction`/`percent`). The EdgionPlugins `RequestMirror` plugin
  embeds the same knobs inline instead (its CRD preserves unknown fields). Invalid
  values log INFO and fall back to the default.

  | Annotation | Default | Meaning |
  |------------|---------|---------|
  | `edgion.io/mirror-connect-timeout-ms` | `1000` | Mirror connect timeout (ms) |
  | `edgion.io/mirror-write-timeout-ms` | `1000` | Mirror write timeout (ms) |
  | `edgion.io/mirror-max-buffered-chunks` | `5` | Max buffered body chunks, `1..=1024` |
  | `edgion.io/mirror-max-buffered-bytes` | `1MiB` | Retained body bytes per mirror, `1B..=1GiB` |
  | `edgion.io/mirror-log` | `true` | Emit a dedicated mirror log line |
  | `edgion.io/mirror-max-concurrent` | `1024` | Max concurrent tasks per plugin instance, `1..=65536`, subject to the process limit |
  | `edgion.io/mirror-channel-full-timeout-ms` | `0` | Wait for channel space (ms); >1000 clamped |

### TCPRoute / TLSRoute / UDPRoute

| Annotation | Value | Description |
|------------|-------|-------------|
| `edgion.io/edgion-stream-plugins` | `name` or `namespace/name` | StreamPlugins binding (TCPRoute / TLSRoute / UDPRoute) |
| `edgion.io/proxy-protocol` | `"v2"` | Send Proxy Protocol v2 to upstream (TLSRoute only; other values ignored) |
| `edgion.io/max-connect-retries` | `u32` | Max upstream connect attempts (TLSRoute only; clamped to 1..=5, default 2) |
| `edgion.io/tcp-keepalive-time` | positive integer string | Idle seconds before the first keepalive probe on the upstream L4 connection (clamped 1–7200). Presence enables keepalive; absent = disabled (TCPRoute / TLSRoute) |
| `edgion.io/tcp-keepalive-interval` | integer string | Seconds between keepalive probes; clamped 1–75; absent or invalid = 75. Only applied when `tcp-keepalive-time` is set (TCPRoute / TLSRoute) |
| `edgion.io/tcp-keepalive-probes` | integer string | Failed probe count before connection is declared dead; clamped 1–20; absent or invalid = 9. Only applied when `tcp-keepalive-time` is set (TCPRoute / TLSRoute) |
| `edgion.io/log-label.<key>` | string | **TLSRoute only.** Adds `<key>=<value>` to `Rsc.labels` for `ssl.log`. The TLS handshake layer has no plugin chain, so it cannot carry an `AccessLogExtern` declaration — this prefix is TLS's only log-projection carrier. |

- On `UDPRoute`, `edgion.io/edgion-stream-plugins` runs only `spec.plugins`
  (connection-context, IP-based, fail-close) when admitting a new UDP session;
  it does not use Pingora's listener `ConnectionFilter`, and there is no TLS stage.
- A Gateway-level `edgion.io/edgion-stream-plugins` attachment does not cover a
  UDP listener because that listener runs as a separate Tokio task rather than a
  Pingora `Service`. Attach the policy to `UDPRoute` for UDP session admission.

### EdgionTls

| Annotation | Value | Description |
|------------|-------|-------------|
| `edgion.io/expose-client-cert` | `"true"\|"false"` | Expose parsed client-certificate information to plugin/session/DSL/auth consumers; does not inject headers automatically |
| `edgion.io/log-label.<key>` | string | **TLS-only.** Adds `<key>=<value>` to `Rsc.labels` for `ssl.log`; same rationale as the TLSRoute row above. |

### Service / EndpointSlice

| Annotation | Value | Description |
|------------|-------|-------------|
| `edgion.io/dns-resolution-mode` | `controller\|gateway\|per-request` | ExternalName resolution authority/cache mode; default `controller` |
| `edgion.io/dns-resolver` | comma/space-separated `ip[:port]` | Service resolver override for both planes |
| `edgion.io/dns-resolver-ref` | `namespace/name` | Service httpdns LinkSys override for both planes |

Active health checks, outlier detection (passive ejection / circuit breaking), and
the per-Service degrade-threshold override are **not** annotations — they are
configured via the `EdgionBackendTrafficPolicy` CRD (`spec.healthCheck`,
`spec.outlierDetection`, `spec.loadBalancer.degradeThreshold`), attached to a
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
| `edgion.io/managed-by` | see below | Provenance marker for a system-managed resource; read by nothing |
| `edgion.io/acme-resource` | resource name | ACME resource association |

### `edgion.io/managed-by` Vocabulary

Provenance only. **Nothing reads this label** — no handler consults it, and the `managed_by`
constants in `edgion-resources/src/constants/labels.rs` have no callers anywhere in the workspace.
Treat it as a marker for a human or a `kubectl get -l` selector, never as an enforcement or
ownership mechanism.

Two values are actually written, both by the service that owns the object's lifecycle, each via
its own local string literal rather than the constants module:

| Value | Written on | By |
|-------|-----------|----|
| `acme` | ACME-issued `Secret` and the auto-generated `EdgionTls` | `edgion-controller/src/services/acme/service.rs`, `.../acme/secret_io.rs` |
| `conf-sync-ca` | conf-sync CA and certificate `Secret`s | `edgion-controller/src/services/conf_sync_ca/secret_io.rs` |

`operator`, `center`, and `controller` are declared in the constants module but written by nothing
and read by nothing. In particular, **do not put this label on an `EdgionConfigData`** — it grants
no ownership and enforces nothing there. That kind is the Center-controlled data overlay with a
single intended writer; see
[`../../01-architecture/05-resources/15-edgion-config-data.md`](../../01-architecture/05-resources/15-edgion-config-data.md)
(Ownership) for why no label-based arbitration exists or is planned.

Constant definitions: `edgion-resources/src/constants/labels.rs` (`managed_by` module).

## System Reserved Keys

The following annotations are managed automatically by the system; **do not set them manually**:

| Annotation | Description |
|------------|-------------|
| `edgion.io/hostname-resolution` | DNS resolution results injected by the Controller |
| `edgion.io/sync-version` | Configuration sync version number |
| `edgion.io/dns-resolved-at` | Controller-written freshness timestamp on synthetic EndpointSlices |

See the [references/](references/) directory for the detailed reference.
