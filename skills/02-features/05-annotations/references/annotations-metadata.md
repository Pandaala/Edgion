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
| `edgion.io/edgion-stream-plugins` | `name` or `namespace/name` | none | Enables Gateway-level connection filtering before HTTP parsing or TLS handshake. Current verified runtime path is in `listener_builder.rs`. |

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

- **生效粒度**：HTTPRoute（应用到该 HTTPRoute 下所有 rules）
- **有效值**：
  - `"true"` —— 强制 raw 转发（盖过全局 `forward_normalized_path` / region_route 默认）
  - `"false"` —— 强制 normalized 转发（盖过 region_route 类的 `preserve_raw_path = true`）
  - 缺省 —— 走默认流程（rewrite 终态 → external_jump → 全局开关 → 默认 normalized）
- **解析时机**：HTTPRoute 加载（`http_route_preparse.rs`）
- **存储位置**：`HTTPRouteRule.parsed_forward_raw_path: Option<bool>`
- **非法值处理**：tracing INFO 日志（含 ns / name / value）+ 当作缺省，不 reject route
- **优先级**：注解最高，盖过 `external_jump.preserve_raw_path` 与全局 `forward_normalized_path`
- **示例**：

  ```yaml
  apiVersion: gateway.networking.k8s.io/v1
  kind: HTTPRoute
  metadata:
    name: legacy-app-raw-path
    namespace: production
    annotations:
      edgion.io/forward-raw-path: "true"   # 上游对 path 字面格式敏感，禁止规范化
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

## TCPRoute / TLSRoute `metadata.annotations`

| Key | Resource | Value | Default | Effect |
|-----|----------|-------|---------|--------|
| `edgion.io/edgion-stream-plugins` | `TCPRoute`, `TLSRoute` | `name` or `namespace/name` | none | Resolves dynamic `EdgionStreamPlugins` lookup key. Short names are expanded with the route namespace. |
| `edgion.io/proxy-protocol` | `TLSRoute` | currently `"v2"` is recognized | disabled | Sends Proxy Protocol v2 to upstream. Other values are ignored. |
| `edgion.io/max-connect-retries` | `TLSRoute` | unsigned integer string | `1` | Max upstream connect attempts. Values below `1` are normalized to `1`. |

Current implementation note:

- I verified active annotation resolution paths for `Gateway`, `TCPRoute`, and `TLSRoute`.
- `UDPRoute` has runtime fields for stream plugin state, but I did not find a matching annotation parser in the current Gateway conf handlers. Treat UDP annotation support as unverified until code lands.

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
