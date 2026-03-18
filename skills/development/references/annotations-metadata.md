# `metadata.annotations` Reference

Use this file when the task is about real Kubernetes object annotations under `metadata.annotations`.

## Verify In Code

- `src/types/constants/annotations.rs`
- `src/core/gateway/runtime/server/listener_builder.rs`
- `src/core/gateway/runtime/store/port_gateway_info.rs`
- `src/core/gateway/routes/tcp/conf_handler_impl.rs`
- `src/core/gateway/routes/tls/conf_handler_impl.rs`
- `src/types/resources/http_route_preparse.rs`
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs`
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/grpc_route.rs`
- `src/core/gateway/backends/health/check/annotation.rs`
- `src/types/resources/edgion_tls.rs`

## Gateway `metadata.annotations`

These are read from `Gateway.metadata.annotations`, not per-listener metadata.

| Key | Value | Default | Effect |
|-----|-------|---------|--------|
| `edgion.io/enable-http2` | `"true"` / `"false"` | enabled | Controls HTTP/2 support for listeners built from this Gateway. For HTTP listeners this enables h2c; for HTTPS listeners it enables ALPN h2. |
| `edgion.io/backend-protocol` | string, commonly `"tcp"` | none | Used by TLS listener handling to choose backend protocol behavior. Current examples use `"tcp"` for TLS termination to raw TCP backends. |
| `edgion.io/http-to-https-redirect` | `"true"` / `"false"` | disabled | On non-TLS listeners, enables redirect before normal HTTP proxying. |
| `edgion.io/https-redirect-port` | integer string | `443` | Target port used when HTTP-to-HTTPS redirect is enabled. |
| `edgion.io/metrics-test-key` | string | none | Test-only metrics correlation key. Consumed by Gateway runtime for integration metrics scenarios. |
| `edgion.io/metrics-test-type` | `lb` / `retry` / `latency` | none | Test-only metrics mode selector. |
| `edgion.io/edgion-stream-plugins` | `name` or `namespace/name` | none | Enables Gateway-level connection filtering before HTTP parsing or TLS handshake. Current verified runtime path is in `listener_builder.rs`. |

Important:

- `metrics-test-*` keys are meant for integration testing and diagnostics, not normal production config.
- `edgion.io/edgion-stream-plugins` is the current key used by the runtime path. Older docs used `edgion.io/stream-plugins`.

## HTTPRoute / GRPCRoute `metadata.annotations`

| Key | Value | Default | Effect |
|-----|-------|---------|--------|
| `edgion.io/max-retries` | unsigned integer string | falls back to `EdgionGatewayConfig.spec.max_retries` | Route-level retry override used when upstream connect fails. `0` disables retries for that route. |
| `edgion.io/hostname-resolution` | controller-generated string | system-managed | Diagnostic annotation describing effective hostname resolution. The controller writes it during parse. Do not hand-author it. |

Notes:

- `edgion.io/max-retries` is parsed in `src/types/resources/http_route_preparse.rs`.
- Runtime precedence is route annotation first, then global config, per `src/core/gateway/routes/http/proxy_http/pg_fail_to_connect.rs`.
- `edgion.io/hostname-resolution` is currently injected on `HTTPRoute` and `GRPCRoute`.

## TCPRoute / TLSRoute `metadata.annotations`

| Key | Resource | Value | Default | Effect |
|-----|----------|-------|---------|--------|
| `edgion.io/edgion-stream-plugins` | `TCPRoute`, `TLSRoute` | `name` or `namespace/name` | none | Resolves dynamic `EdgionStreamPlugins` lookup key. Short names are expanded with the route namespace. |
| `edgion.io/proxy-protocol` | `TLSRoute` | currently `"v2"` is recognized | disabled | Sends Proxy Protocol v2 to upstream. Other values are ignored. |
| `edgion.io/upstream-tls` | `TLSRoute` | `"true"` / `"false"` | `false` | Enables TLS when connecting from Gateway to backend. |
| `edgion.io/max-connect-retries` | `TLSRoute` | unsigned integer string | `1` | Max upstream connect attempts. Values below `1` are normalized to `1`. |

Current implementation note:

- I verified active annotation resolution paths for `Gateway`, `TCPRoute`, and `TLSRoute`.
- `UDPRoute` has runtime fields for stream plugin state, but I did not find a matching annotation parser in the current Gateway conf handlers. Treat UDP annotation support as unverified until code lands.

## EdgionTls `metadata.annotations`

| Key | Value | Default | Effect |
|-----|-------|---------|--------|
| `edgion.io/expose-client-cert` | `"true"` / `"false"` | disabled | When mTLS is enabled, exposes parsed client certificate info to the plugin/session layer. |

## Service / EndpointSlice / Endpoints `metadata.annotations`

| Key | Value | Default | Effect |
|-----|-------|---------|--------|
| `edgion.io/health-check` | YAML string | none | Configures active backend health checks. Parsed from object metadata and validated before use. |

Notes:

- The parser accepts YAML text, not flat key-value options.
- The user-facing health-check guide remains the best place for schema examples:
  - `docs/zh-CN/user-guide/http-route/backends/health-check.md`
  - `docs/en/user-guide/http-route/backends/health-check.md`

## Legacy Key Warning

Older repo content may still show:

- `edgion.io/stream-plugins`

For new work, prefer:

- `edgion.io/edgion-stream-plugins`

If you touch an old example or doc, update it instead of copying the legacy key forward.
