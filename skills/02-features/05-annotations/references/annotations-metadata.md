# `metadata.annotations` Reference

Use this file when the task is about real Kubernetes object annotations under `metadata.annotations`.

## Verify In Code

- `src/types/constants/annotations.rs`
- `src/core/gateway/runtime/server/listener_builder.rs`
- `src/core/gateway/runtime/store/listener_gateway_info.rs`
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
| `edgion.io/enable-http2` | `"true"` / `"false"` | enabled | **Frontend (Listener-side) only.** Controls HTTP/2 support for listeners built from this Gateway. For HTTP listeners this enables h2c; for HTTPS listeners it enables ALPN h2. When disabled, pure gRPC requests are rejected with 500. Does not affect backend protocol selection (use Service `appProtocol` for that). |
| `edgion.io/http-to-https-redirect` | `"true"` / `"false"` | disabled | On non-TLS listeners, enables redirect before normal HTTP proxying. |
| `edgion.io/https-redirect-port` | integer string | `443` | Target port used when HTTP-to-HTTPS redirect is enabled. |
| `edgion.io/metrics-test-key` | string | none | Test-only metrics correlation key. Consumed by Gateway runtime for integration metrics scenarios. |
| `edgion.io/metrics-test-type` | `lb` / `retry` / `latency` | none | Test-only metrics mode selector. |
| `edgion.io/edgion-stream-plugins` | `name` or `namespace/name` | none | Enables Gateway-level connection filtering before HTTP parsing or TLS handshake. Short names are expanded with the Gateway's namespace. Current verified runtime path is in `listener_builder.rs`. |
| `edgion.io/require-sni-host-match` | `"true"` / `"false"` | inherits `EdgionGatewayConfig.spec.requireSniHostMatch` | When `true`, on HTTPS listeners the SNI must match the request `Host` header (otherwise the request is rejected). Read in `listener_gateway_info.rs`; falls back to the cluster-wide `EdgionGatewayConfig` setting when absent. |

Important:

- `metrics-test-*` keys are meant for integration testing and diagnostics, not normal production config.
- `edgion.io/edgion-stream-plugins` is the current key used by the runtime path. Older docs used `edgion.io/stream-plugins`.

## HTTPRoute / GRPCRoute `metadata.annotations`

| Key | Value | Default | Effect |
|-----|-------|---------|--------|
| `edgion.io/hostname-resolution` | controller-generated string | system-managed | Diagnostic annotation describing effective hostname resolution. The controller writes it during parse. Do not hand-author it. |

Notes:

- Retry configuration uses the standard Gateway API `spec.rules[*].retry` field (see HTTPRoute/GRPCRoute resource docs). Runtime precedence: `rule.retry.attempts > EdgionGatewayConfig.spec.maxRetries`.
- `edgion.io/hostname-resolution` is currently injected on `HTTPRoute` and `GRPCRoute`.

### `edgion.io/forward-raw-path`

- **Granularity**: HTTPRoute (applied to all rules under this HTTPRoute).
- **Valid values**:
  - `"true"` — force raw forwarding (overrides the global `forward_normalized_path` / region_route default).
  - `"false"` — force normalized forwarding (overrides region_route-style `preserve_raw_path = true`).
  - absent — use the default flow (rewrite final state → external_jump → global switch → normalized by default).
- **Parsing time**: HTTPRoute load (`http_route_preparse.rs`).
- **Storage**: `HTTPRouteRule.parsed_forward_raw_path: Option<bool>`.
- **Invalid value handling**: tracing INFO log (including ns / name / value) + treat as absent; the route is not rejected.
- **Priority**: the annotation has the highest priority, overriding `external_jump.preserve_raw_path` and the global `forward_normalized_path`.
- **Example**:

  ```yaml
  apiVersion: gateway.networking.k8s.io/v1
  kind: HTTPRoute
  metadata:
    name: legacy-app-raw-path
    namespace: production
    annotations:
      edgion.io/forward-raw-path: "true"   # Upstream is sensitive to literal path format; normalization is forbidden
  spec:
    rules:
      - matches:
          - path:
              type: PathPrefix
              value: /legacy
        backendRefs:
          - name: legacy-server
            port: 8080
  ```

## TCPRoute / TLSRoute / UDPRoute `metadata.annotations`

| Key | Resource | Value | Default | Effect |
|-----|----------|-------|---------|--------|
| `edgion.io/edgion-stream-plugins` | `TCPRoute`, `TLSRoute`, `UDPRoute` | `name` or `namespace/name` | none | Resolves dynamic `EdgionStreamPlugins` lookup key. Short names are expanded with the route namespace. On `UDPRoute` only the Stage-1 ConnectionFilter (`spec.plugins`, IP-based) runs — there is no TLS stage. |
| `edgion.io/proxy-protocol` | `TLSRoute` | currently `"v2"` is recognized | disabled | Sends Proxy Protocol v2 to upstream. Other values are ignored. |
| `edgion.io/max-connect-retries` | `TLSRoute` | unsigned integer string | `2` | Max upstream connect attempts. Values below `1` are normalized to `1`; values above `5` are capped to `5`. |

Current implementation note:

- Active annotation resolution paths verified for `Gateway`, `TCPRoute`, `TLSRoute`, and `UDPRoute`.
- `UDPRoute` resolves the key in `GlobalUdpRouteManagers::initialize_route` and enforces it (fail-close) in `edgion_udp.rs::handle_client_packet`, mirroring the TCP path. The F-016 session fast-path returns first for established sessions, so the check runs once at session creation (a denied client never establishes a session) — like TCP's per-connection check. Only Stage-1 plugins apply (UDP has no TLS handshake).

## EdgionTls `metadata.annotations`

| Key | Value | Default | Effect |
|-----|-------|---------|--------|
| `edgion.io/expose-client-cert` | `"true"` / `"false"` | disabled | When mTLS is enabled, exposes parsed client certificate info to the plugin/session layer. |

## Service `spec.ports[].appProtocol` — Backend Protocol Selection (GEP-1911)

Not an `edgion.io/*` annotation, but a standard Kubernetes field that Edgion reads for backend protocol selection. This is the **backend-side** counterpart to the frontend-side `edgion.io/enable-http2` annotation.

| Value | Effect |
|-------|--------|
| `kubernetes.io/h2c` or `h2c` | Forces HTTP/2 cleartext to upstream (`set_http_version(2, 2)`) |
| `kubernetes.io/ws`, `ws`, `kubernetes.io/wss`, `wss` | Forces HTTP/1.1 for WebSocket upgrade (`set_http_version(1, 1)`) |

Verify in code: `src/core/gateway/backends/mod.rs` (`detect_backend_app_protocol`, `apply_backend_app_protocol`).

GatewayClass declares `HTTPRouteBackendProtocolH2C` as a supported feature.

## Service / EndpointSlice / Endpoints `metadata.annotations`

| Key | Value | Default | Effect |
|-----|-------|---------|--------|
| `edgion.io/health-check` | YAML string | none | Configures active backend health checks (type: http/tcp/grpc). For gRPC, supports `grpcServiceName` field. Parsed from object metadata and validated before use. |
| `edgion.io/circuit-breaker` | YAML string | none | Configures backend outlier-detection circuit breaking. Fields: `consecutiveErrors`, `ejectionSeconds`, `maxEjectionPercent`. Verify in code: `src/core/gateway/backends/circuit_breaker/config.rs`. |

Notes:

- The parser accepts YAML text, not flat key-value options.
- Supported probe types: `http` (default), `tcp`, `grpc` (gRPC Health Check Protocol).
- The user-facing health-check guide remains the best place for schema examples:
  - `docs/zh-CN/user-guide/http-route/backends/health-check.md`
  - `docs/en/user-guide/http-route/backends/health-check.md`

## Legacy Key Warning

Older repo content may still show:

- `edgion.io/stream-plugins`

For new work, prefer:

- `edgion.io/edgion-stream-plugins`

If you touch an old example or doc, update it instead of copying the legacy key forward.
