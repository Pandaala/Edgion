# `metadata.annotations` Reference

Use this file when the task is about real Kubernetes object annotations under `metadata.annotations`.

## Verify In Code

- `edgion-resources/src/constants/annotations.rs`
- `edgion-gateway/src/runtime/server/base.rs`
- `edgion-gateway/src/runtime/gateway_info.rs`
- `edgion-gateway/src/routes/tcp/conf_handler_impl.rs`
- `edgion-gateway/src/routes/tls/conf_handler_impl.rs`
- `edgion-resources/src/resources/http_route_preparse.rs`
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs`
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/grpc_route.rs`
- `edgion-resources/src/resources/edgion_tls.rs`

## Gateway `metadata.annotations`

These are read from `Gateway.metadata.annotations`, not per-listener metadata.

| Key | Value | Default | Effect |
|-----|-------|---------|--------|
| `edgion.io/enable-http2` | string | enabled | Frontend only. Case-insensitive `false`/`0`/`no`/`off` disables; any other present value enables. HTTP listeners use h2c and HTTPS listeners advertise ALPN h2. Backend protocol uses Service `appProtocol`. |
| `edgion.io/http-to-https-redirect` | string | disabled | On non-TLS listeners, only case-insensitive `true` enables redirect. |
| `edgion.io/https-redirect-port` | integer string | `443` | Target port used when HTTP-to-HTTPS redirect is enabled. |
| `edgion.io/metrics-test-key` | string | none | Test-only metrics correlation key. Consumed by Gateway runtime for integration metrics scenarios. |
| `edgion.io/metrics-test-type` | `lb` / `retry` / `latency` | none | Test-only metrics mode selector. |
| `edgion.io/edgion-stream-plugins` | `name` | none | Enables Gateway-level connection filtering before HTTP parsing or TLS handshake. The resource is resolved in the Gateway's namespace; slash-containing values are rejected by the Controller and fail closed defensively in the Gateway. Current verified runtime path is in `listener_builder.rs`. |
| `edgion.io/edgion-plugins` | comma/space-separated refs | none | Ordered HTTP plugins applied to every request on this Gateway before route plugins. Bare names use the Gateway namespace; duplicates preserve first occurrence. |
| `edgion.io/require-sni-host-match` | `"true"` / `"false"` | inherits `EdgionGatewayConfig.spec.securityProtect.requireSniHostMatch` | Explicit true or false overrides the global value for this Gateway; absent inherits. HTTPS mismatch returns 421. |
| `edgion.io/dns-resolver` | `ip[:port]` list (comma/space-separated) | inherits `EdgionGatewayConfig.spec.dnsResolver` | Per-Gateway inline standard-DNS resolver for `ServiceExternalName` backends. **Data-plane only** (D3a): overrides the EGC default but not control-plane resolution. Mutually exclusive with `dns-resolver-ref`. |
| `edgion.io/dns-resolver-ref` | `namespace/name` | inherits `EdgionGatewayConfig.spec.dnsResolver` | Per-Gateway httpdns LinkSys resolver reference. Explicit namespace required (bare names rejected). **Data-plane only**. A cross-namespace ref is ReferenceGrant-gated (from-kind `Gateway`, to-kind `edgion.io/LinkSys`); a denied ref is reported via `ResolvedRefs=False` and stripped before it reaches the data plane. Mutually exclusive with `dns-resolver`. |

| `edgion.io/inbound-proxy-protocol` | strict tagged JSON; see below | disabled | Receives PROXY protocol v1/v2 on selected TCP-based Gateway listeners. The Controller resolves it once and the Gateway consumes only the typed result. Invalid input never falls back to a wider trust mode. |

Important:

- `metrics-test-*` keys are meant for integration testing and diagnostics, not normal production config.
- `edgion.io/edgion-stream-plugins` is the current key used by the runtime path. Older docs used `edgion.io/stream-plugins`.
- `edgion.io/inbound-proxy-protocol` (inbound, on `Gateway`) and `edgion.io/proxy-protocol` (outbound, on `TLSRoute`) are opposite directions and unrelated.
- Inline shapes are `{"source":"inline","listeners":[...],"config":{"mode":"required"}}` and `{"source":"inline","listeners":[...],"config":{"mode":"trustedSources","trustedCidrs":[...]}}`.
- The reference shape is `{"source":"resourceRef","listeners":[...],"resourceRef":{"name":"NAME","namespace":"NAMESPACE"}}`. `listeners` and `namespace` are optional; every other field is strict.
- Omitted `listeners` selects all TCP-based listeners and ignores UDP on a mixed Gateway. A supplied list contains 1–64 unique existing HTTP/HTTPS/TCP/TLS listener names. Explicit UDP selection and UDP-only Gateways are invalid.
- `required` accepts a header from every peer and rejects missing headers. `trustedSources` parses a header only for a `DirectPeerIp` matching one of 1–256 canonical CIDRs; other peers remain direct connections. An accepted header supplies `RemoteIp` but never rewrites `DirectPeerIp`.
- Bare trusted addresses normalize to `/32` or `/128`; invalid entries, normalized duplicates, and `/0` reject the complete policy. No partial matcher or trust-all fallback is built.
- A referenced `EdgionConfigData` must be enabled, have no active overlay, use `data.type: ProxyProtocolTrust`, and contain the same policy schema. When EdgionConfigData reference validation is disabled, the reference is allowed. With validation enabled, same-namespace references are always allowed; a cross-namespace target with `visibility: Cluster` is allowed without a grant; a cross-namespace target with `visibility: Namespace` requires an exact `ReferenceGrant` from `gateway.networking.k8s.io/Gateway` to `edgion.io/EdgionConfigData`.

  ```yaml
  apiVersion: edgion.io/v1
  kind: EdgionConfigData
  metadata:
    name: edge-load-balancers
    namespace: networking
  spec:
    enable: true
    visibility: Namespace
    data:
      type: ProxyProtocolTrust
      config:
        mode: trustedSources
        trustedCidrs: [10.0.0.0/8, 192.168.1.5]
  ```

- Invalid JSON, policy, listener selection, reference, or shared physical-listener policy produces explicit invalid status and prevents Pingora startup. Any effective post-start socket-policy change trips a monotonic guard: readiness becomes 503, new TCP connections are rejected, and the Gateway exits non-zero for supervisor restart.
- A PROXY header is unauthenticated. A trusted endpoint must generate it itself; a transparent relay can launder client-authored bytes and must not be trusted.
- `edgion.io/edgion-stream-plugins` runs at accept time, **before** the PROXY header is parsed, so its IP rules match the load balancer rather than the real client. The gateway logs a warning when both are enabled on one listener.
- Gateway-level StreamPlugins attachments are intentionally same-namespace. To reuse centrally managed lists or selectors across per-namespace policies, reference a shared `EdgionConfigData` from each local `EdgionStreamPlugins`; those cross-namespace data references use the existing `ReferenceGrant` and `visibility` controls.

## Log Projection Annotations

| Key | Scope | Effect |
|-----|-------|--------|
| `edgion.io/log-label.<key>` | **TLSRoute, EdgionTls only** | Adds the suffix/value to resolved `Rsc.labels`, consumed by `ssl.log`. The same prefix may also be used on `metadata.labels`. **Not honored on HTTPRoute / GRPCRoute** — the TLS handshake layer has no plugin chain to attach a declaration to, so this prefix remains its only log-projection carrier. |

The old comma-separated ctx-key allowlist annotation and the old unified
JSON log-projection annotation have both been **removed entirely** (no
aliases, no migration shim). On HTTPRoute / GRPCRoute, all log projection —
route label/annotation values, request/response fields, plugin-set ctx vars —
now goes through an `AccessLogExtern` declaration
(`EdgionPlugins.spec.accessLogExtern`, attached via an `ExtensionRef` filter),
which lands the collected fields in a nested `ext` block instead of flattening
them at the entry's top level. See
`skills/03-coding/observability/00-access-log.md` and
`docs/superpowers/specs/2026-07-16-access-log-extern-plugin-design.md`.

The `log-label.<key>` prefix on TLSRoute/EdgionTls is resolved by
`resolve_log_labels` in `edgion-resources/src/resources/common/log_labels.rs`
(capped at 16 entries, 63 bytes per key, 256 bytes per value); the function's
JSON-array-parsing half (for the deleted unified log-projection annotation)
has been removed.

## HTTPRoute / GRPCRoute `metadata.annotations`

| Key | Value | Default | Effect |
|-----|-------|---------|--------|
| `edgion.io/hostname-resolution` | controller-generated string | system-managed | Diagnostic annotation describing effective hostname resolution. The controller writes it during parse. Do not hand-author it. |
| `edgion.io/retry-non-idempotent` | `"true"` / `"false"` | `false` (safe) | Opt-in to retrying non-idempotent methods on the reused-connection retry path. Applies to all rules under this HTTPRoute/GRPCRoute. |
| `edgion.io/mirror-connect-timeout-ms` | integer string | `1000` | RequestMirror connect timeout (ms). Applies to all RequestMirror filters under this HTTPRoute. |
| `edgion.io/mirror-write-timeout-ms` | integer string | `1000` | RequestMirror write timeout (ms). |
| `edgion.io/mirror-max-buffered-chunks` | integer string | `5` | Max body chunks buffered per mirror, `1..=1024`; invalid values use the default. |
| `edgion.io/mirror-max-buffered-bytes` | byte-size string | `1MiB` | Retained body bytes per mirror, `1B..=1GiB`; invalid values use the default. |
| `edgion.io/mirror-log` | `"true"` / `"false"` | `true` | Whether the mirror emits a dedicated mirror-log line. |
| `edgion.io/mirror-max-concurrent` | integer string | `1024` | Max concurrent mirror tasks per plugin instance, `1..=65536`, subject to the shared process limit; invalid values use the default. |
| `edgion.io/mirror-channel-full-timeout-ms` | integer string | `0` | Max ms to wait for mirror channel space before abandoning the mirror; values above `1000` are clamped. |

Notes:

- Retry configuration uses the standard Gateway API `spec.rules[*].retry` field (see HTTPRoute/GRPCRoute resource docs). Runtime precedence: `rule.retry.attempts > EdgionGatewayConfig.spec.retry.attempts`.
- `edgion.io/hostname-resolution` is currently injected on `HTTPRoute` and `GRPCRoute`.

### `edgion.io/retry-non-idempotent`

- **Granularity**: HTTPRoute / GRPCRoute (applied to all rules under the route).
- **Valid values**:
  - `"true"` — allow the reused-connection retry to replay a non-idempotent request body (restores the pre-guard behavior).
  - `"false"` / absent / any other value — keep the safe default.
- **Default behavior (annotation absent)**: when a reused keepalive connection is reset mid-request, the buffered request body is replayed on a fresh connection only for idempotent methods (GET/HEAD/PUT/DELETE/OPTIONS/TRACE). Non-idempotent methods (POST/PATCH/CONNECT, and all gRPC since it is POST at the HTTP layer) are **not** retried there, to avoid a silent duplicate mutation.
- **Scope**: only the reused-connection retry path (`error_while_proxy` Case 2). The connect-failure path (`fail_to_connect`) never wrote request bytes upstream and is always retried regardless of method. The status-code retry (`retry.codes`) and per-try-timeout paths are explicitly operator-configured and are unaffected by this guard.
- **Why an annotation, not a CRD field**: the standard Gateway API `HTTPRouteRetry` CRD is a structural schema that prunes unknown fields, so a new `spec.rules[*].retry` field would be silently dropped by the API server.
- **Idempotency definition**: uses `http::Method::is_idempotent()` (RFC 9110), matching nginx's default `proxy_next_upstream` exclusion of non-idempotent methods.

### `edgion.io/mirror-*` (RequestMirror tuning)

- **Granularity**: HTTPRoute (applied to every `RequestMirror` filter under the route,
  both rule-level and backendRef-level filters). Under delegation, the parent route's
  annotations apply to the flattened `resolved_rules`; a child route's own mirror-*
  annotations are not carried over (same semantics as `forward-raw-path` /
  `retry-non-idempotent`).
- **Why annotations, not filter fields**: Edgion installs the unmodified vendored
  Gateway API CRD, a structural schema — in K8s conf-center mode the API server
  silently prunes any `requestMirror` field the schema does not declare (no admission
  error). The `HTTPRequestMirrorFilter` struct therefore keeps only the standard
  `backendRef` / `fraction` / `percent` fields; do NOT add extension fields to it.
- **Parsing time**: HTTPRoute load (`http_route_preparse.rs`,
  `parse_mirror_tuning_annotations`).
- **Storage**: `HTTPRouteRule.parsed_mirror_tuning: Option<MirrorTuning>` (copied to
  every rule, `None` when no mirror annotation is present → defaults apply).
- **Invalid value handling**: tracing INFO log + fall back to that field's default;
  the route is not rejected and the other keys still apply.
- **EdgionPlugins path**: the `RequestMirror` plugin variant embeds the same knobs
  inline (`RequestMirrorPluginConfig`, flattened standard + tuning fields) because the
  EdgionPlugins CRD preserves unknown fields — no pruning, no annotations needed.

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
| `edgion.io/edgion-stream-plugins` | `TCPRoute`, `TLSRoute`, `UDPRoute` | `name` or `namespace/name` | none | Resolves dynamic `EdgionStreamPlugins` lookup key. Short names are expanded with the route namespace. TCP/UDP routes execute `spec.plugins`; TLSRoute executes only `spec.tlsRoutePlugins`. A TLSRoute reference to a nonempty Stage-1-only resource fails closed; explicit empty Stage-2 is a no-op. |
| `edgion.io/proxy-protocol` | `TLSRoute` | currently `"v2"` is recognized | disabled | Sends Proxy Protocol v2 to upstream. Other values are ignored. |
| `edgion.io/max-connect-retries` | `TLSRoute` | unsigned integer string | `2` | Max upstream connect attempts. Values below `1` are normalized to `1`; values above `5` are capped to `5`. |
| `edgion.io/tcp-keepalive-time` | `TCPRoute`, `TLSRoute` | positive integer string | disabled (absent) | Idle seconds before the first TCP keepalive probe on the upstream L4 connection. Presence (a valid positive integer) enables keepalive; absent disables it entirely (no probes sent). Clamped to 1–7200; out-of-range values are clamped with a warning at config-load time. Mitigates half-open connections after NAT/firewall drops (~15 min silent stall without keepalive). |
| `edgion.io/tcp-keepalive-interval` | `TCPRoute`, `TLSRoute` | integer string | `75` (OS default) | Seconds between consecutive keepalive probes. Only applied when `tcp-keepalive-time` is also set. Clamped to 1–75; absent or invalid value falls back to `75`. |
| `edgion.io/tcp-keepalive-probes` | `TCPRoute`, `TLSRoute` | integer string | `9` (OS default) | Number of consecutive failed probes before the connection is declared dead and closed. Only applied when `tcp-keepalive-time` is also set. Clamped to 1–20; absent or invalid value falls back to `9`. |

Current implementation note:

- Active annotation resolution paths verified for `Gateway`, `TCPRoute`, `TLSRoute`, and `UDPRoute`; attachment lifecycle still differs by protocol.
- `UDPRoute` resolves the key in `UdpRouteUnit::from_route` and enforces it (fail-close) in `edgion_udp.rs::handle_client_packet`. The F-016 session fast-path returns first for established sessions, so the check runs once at session creation (a denied client never establishes a session), not once per datagram. Only `spec.plugins` applies because UDP has no TLS handshake.
- A Gateway-level attachment does not cover a UDP listener: `add_udp_listener` runs an independent Tokio task and never installs Pingora's listener `ConnectionFilter`. Attach the resource to `UDPRoute` for UDP session admission.

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

Verify in code: `edgion-gateway/src/backends/mod.rs` (`detect_backend_app_protocol`, `apply_backend_app_protocol`).

GatewayClass declares `HTTPRouteBackendProtocolH2C` as a supported feature.

## Service / EndpointSlice `metadata.annotations`

| Key | Value | Default | Effect |
|-----|-------|---------|--------|
| `edgion.io/dns-resolution-mode` | `controller` / `gateway` / `per-request` | `controller` | Per-`ServiceExternalName` DNS resolution mode. `controller`: use the controller-resolved EndpointSlice when it has a ready backend, else the gateway bridges via self-resolution + 5s cache. `gateway`: gateway resolves via the effective resolver + 5s cache (authoritative). `per-request`: resolve on every request, no cache. Invalid values fall back to `controller`. |
| `edgion.io/dns-resolver` | `ip[:port]` list (comma/space-separated) | inherits Gateway / EGC / system | Per-Service inline standard-DNS resolver (most specific; applies to **both** planes). Mutually exclusive with `dns-resolver-ref`. |
| `edgion.io/dns-resolver-ref` | `namespace/name` | inherits Gateway / EGC / system | Per-Service httpdns LinkSys resolver reference (most specific; applies to **both** planes). Explicit namespace required (bare names rejected). A cross-namespace ref is ReferenceGrant-gated (from-kind `Service`, to-kind `edgion.io/LinkSys`); a denied ref is logged and stripped before it reaches the data plane. Mutually exclusive with `dns-resolver`. |

Active health checks, outlier detection (passive ejection / circuit breaking), and
the per-Service degrade-threshold override are **not** Service annotations anymore —
they moved to the `EdgionBackendTrafficPolicy` CRD (`spec.healthCheck` /
`spec.outlierDetection` / `spec.loadBalancer.degradeThreshold`, attached via
`targetRefs`). See
[03-resources/20-edgion-backend-traffic-policy.md](../../03-resources/20-edgion-backend-traffic-policy.md)
for the schema and `edgion-resources/src/resources/edgion_backend_traffic_policy.rs`
for the implementation.

Notes:

- Supported active-probe types: `http` (default), `tcp`, `grpc` (gRPC Health Check Protocol).

## Legacy Key Warning

Older repo content may still show:

- `edgion.io/stream-plugins`

For new work, prefer:

- `edgion.io/edgion-stream-plugins`

If you touch an old example or doc, update it instead of copying the legacy key forward.
