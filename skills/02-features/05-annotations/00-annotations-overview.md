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
| `edgion.io/log-label.<key>` | string | Adds `<key>=<value>` to `rsc.labels` in the access log (also on `metadata.labels`). |
| `edgion.io/log-ctx-keys` | comma-separated list | Opt-in allowlist of plugin-set ctx-var (`ctx_map`) keys to project into the access log. Context variables are **not** logged by default; only keys named here are emitted (known-sensitive keys like claims stay hard-blocked). Shorthand for `log-keys` entries of `type: ctx`. |
| `edgion.io/log-keys` | JSON array | Unified opt-in log-projection carrier. Each entry is `{"key": "<name>", "type": "ctx"\|"label"\|"annotation"}`. `type: ctx` adds `<name>` to the access-log ctx allowlist; `type: label` / `type: annotation` projects the value of the existing `metadata.labels[<name>]` / `metadata.annotations[<name>]` into `rsc.labels`. Malformed entries are skipped individually; merges with `log-label.<key>` and `log-ctx-keys`. |

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
| `edgion.io/expose-client-cert` | `"true"\|"false"` | Expose client certificate to request headers |

### Service / EndpointSlice / Endpoints

| Annotation | Value | Description |
|------------|-------|-------------|
| `edgion.io/health-check` | YAML string | Health check configuration |
| `edgion.io/circuit-breaker` | YAML string | Circuit breaker configuration (consecutiveErrors/ejectionSeconds/maxEjectionPercent) |
| `edgion.io/panic-threshold` | integer string `0`–`100` | Panic threshold percent (default `0` = disabled). When the healthy-backend fraction drops below this value, the gateway routes across ALL backends instead of returning 503. Overrides the global `EdgionGatewayConfig.spec.loadBalancing.panicThreshold`. Prometheus gauge: `edgion_lb_panic_mode{service}` = 1 when engaged. |

> **Panic threshold and the circuit breaker / health-check safety valve.** `edgion.io/panic-threshold` acts as a last-resort layer on top of the combined health-check + circuit-breaker predicate: when both active probes and passive ejection have together removed too many backends, panic mode kicks in and prevents a total 503 outage. The three annotations work best together on services with larger backend pools.

> **When to enable panic threshold.** Default is `0` (disabled), matching Istio's
> Kubernetes default, because a 50% threshold misfires on small pools — a 2-pod
> service losing 1 pod hits 50% and would route back to the bad pod. Enable it
> (e.g. `50`) for services with **larger** backend pools (≈ ≥ 5 endpoints) where
> losing many backends at once more likely means a systemic problem (a
> misconfigured health-check path, a network partition) than genuine failure.
> The canonical case: a renamed health-check endpoint marks every backend
> unhealthy and, without panic mode, takes the whole service offline with 503s.

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

See the [references/](references/) directory for the detailed reference.
