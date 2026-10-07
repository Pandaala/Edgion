//! Gateway API and Edgion annotation constants

/// Edgion-specific annotations
pub mod edgion {
    /// Reserved Controller-resolved transport marker for ConfigMap CA material.
    /// Ordinary fetched Secrets have this marker removed before embedding.
    pub const CONTROLLER_CA_SOURCE: &str = "edgion.io/controller-ca-source";
    /// Exact source kind carried by [`CONTROLLER_CA_SOURCE`].
    pub const CONTROLLER_CA_SOURCE_CONFIG_MAP: &str = "ConfigMap";
    /// Gateway-local traffic socket IP, independent of Gateway API external addresses.
    pub const LOCAL_BIND_IP: &str = "edgion.io/local-bind-ip";
    /// Service annotation naming the Gateway whose external address it serves.
    /// The value is `namespace/name`; the matching Gateway UID is required too.
    pub const GATEWAY_SERVICE_REF: &str = "edgion.io/gateway-service-ref";
    /// Service annotation binding an ingress Service to one Gateway instance.
    pub const GATEWAY_SERVICE_UID: &str = "edgion.io/gateway-service-uid";
    /// Gateway annotation requiring its UID-bound dedicated ingress Service.
    pub const DEDICATED_GATEWAY_INSTANCE: &str = "edgion.io/dedicated-gateway-instance";
    /// Enable HTTP/2 support on the listener
    pub const ENABLE_HTTP2: &str = "edgion.io/enable-http2";
    /// Enable automatic HTTP to HTTPS redirect
    pub const HTTP_TO_HTTPS_REDIRECT: &str = "edgion.io/http-to-https-redirect";
    /// Target port for HTTPS redirect
    pub const HTTPS_REDIRECT_PORT: &str = "edgion.io/https-redirect-port";
    /// Expose mTLS client certificate info to plugin layer via TLS digest extension
    pub const EXPOSE_CLIENT_CERT: &str = "edgion.io/expose-client-cert";

    /// Proxy Protocol version to send to upstream.
    /// Value: "v2" to enable PP2.
    pub const PROXY_PROTOCOL: &str = "edgion.io/proxy-protocol";

    /// Strict JSON configuration for inbound PROXY protocol parsing on a
    /// Gateway. This is unrelated to [`PROXY_PROTOCOL`], which configures the
    /// outbound Gateway-to-upstream direction.
    pub const INBOUND_PROXY_PROTOCOL: &str = "edgion.io/inbound-proxy-protocol";

    /// Per-route opt-out / opt-in for upstream path forwarding view.
    /// Three-state on HTTPRoute.metadata.annotations:
    ///   "true"  — force raw forward (overrides global forward_normalized_path / region_route default)
    ///   "false" — force normalized forward (overrides region_route's preserve_raw_path)
    ///   absent  — fall through to default flow
    /// Invalid values are logged at INFO and treated as absent.
    pub const FORWARD_RAW_PATH: &str = "edgion.io/forward-raw-path";

    /// Max upstream connect retries for TLS/TCP routes.
    /// Value: unsigned integer string (default: 1, i.e. no retry).
    /// Gateway API TLSRoute has no retry spec, so this is Edgion-specific.
    pub const MAX_CONNECT_RETRIES: &str = "edgion.io/max-connect-retries";

    /// Per-route opt-in to retry non-idempotent methods (POST/PATCH/CONNECT,
    /// and all gRPC since it is POST at the HTTP layer) on the reused-connection
    /// retry path. Two-state on HTTPRoute/GRPCRoute.metadata.annotations:
    ///   "true"  — allow the reused-connection retry to replay a non-idempotent
    ///             request body (matches the pre-guard behavior).
    ///   absent / other — keep the safe default: a reused keepalive connection
    ///             reset mid-request is NOT retried for non-idempotent methods,
    ///             because replaying the buffered body risks a silent duplicate
    ///             mutation. Idempotent methods (GET/HEAD/PUT/DELETE/OPTIONS/
    ///             TRACE) are unaffected and still retried.
    /// Invalid values are logged at INFO and treated as absent (safe default).
    /// The Gateway API HTTPRouteRetry CRD is a standard structural schema that
    /// prunes unknown fields, so this knob lives in an annotation rather than a
    /// new spec field.
    pub const RETRY_NON_IDEMPOTENT: &str = "edgion.io/retry-non-idempotent";

    /// RequestMirror tuning annotations on HTTPRoute.metadata.annotations,
    /// applied to every RequestMirror filter under the route. These knobs live
    /// in annotations rather than on `HTTPRequestMirrorFilter` because the
    /// vendored Gateway API CRD is a structural schema that silently prunes
    /// undeclared filter fields in K8s conf-center mode. Invalid values are
    /// logged at INFO and fall back to the field default.
    ///
    /// Connection timeout for the mirror backend, in milliseconds
    /// (default: 1000).
    pub const MIRROR_CONNECT_TIMEOUT_MS: &str = "edgion.io/mirror-connect-timeout-ms";
    /// Write/send timeout for the mirror backend, in milliseconds
    /// (default: 1000).
    pub const MIRROR_WRITE_TIMEOUT_MS: &str = "edgion.io/mirror-write-timeout-ms";
    /// Max milliseconds to wait for channel space when the mirror body channel
    /// is full before abandoning the mirror (default: 0 = abandon immediately;
    /// the gateway clamps values above 1000).
    pub const MIRROR_CHANNEL_FULL_TIMEOUT_MS: &str = "edgion.io/mirror-channel-full-timeout-ms";
    /// Max number of body chunks buffered for the mirror when it is slower
    /// than the main request (default: 5; range: 1..=1024).
    pub const MIRROR_MAX_BUFFERED_CHUNKS: &str = "edgion.io/mirror-max-buffered-chunks";
    /// Maximum retained body bytes per mirror (default: 1MiB; range: 1B..=1GiB).
    pub const MIRROR_MAX_BUFFERED_BYTES: &str = "edgion.io/mirror-max-buffered-bytes";
    /// Whether the mirror emits a dedicated mirror-log line: "true" / "false"
    /// (default: true).
    pub const MIRROR_LOG: &str = "edgion.io/mirror-log";
    /// Max concurrent mirror tasks per plugin instance (default: 1024;
    /// range: 1..=65536), subject to the shared process limit.
    pub const MIRROR_MAX_CONCURRENT: &str = "edgion.io/mirror-max-concurrent";

    /// Idle seconds before the first TCP keepalive probe on L4 upstream
    /// connections. Presence of this annotation (parsed to a positive integer)
    /// is the enablement trigger for upstream keepalive; absent = disabled.
    /// Value: integer seconds, clamped to 1-7200.
    pub const TCP_KEEPALIVE_TIME: &str = "edgion.io/tcp-keepalive-time";

    /// Seconds between TCP keepalive probes on L4 upstream connections.
    /// Only applied when `TCP_KEEPALIVE_TIME` is set. Value: integer seconds,
    /// clamped to 1-75; absent = 75 (OS default).
    pub const TCP_KEEPALIVE_INTERVAL: &str = "edgion.io/tcp-keepalive-interval";

    /// Number of failed TCP keepalive probes before the upstream connection is
    /// considered dead. Only applied when `TCP_KEEPALIVE_TIME` is set.
    /// Value: integer, clamped to 1-20; absent = 9 (OS default).
    pub const TCP_KEEPALIVE_PROBES: &str = "edgion.io/tcp-keepalive-probes";

    /// Sync version injected by gateway after gRPC sync.
    /// Used for correlating data-plane logs with control-plane events.
    pub const SYNC_VERSION: &str = "edgion.io/sync-version";

    // ========== Test metrics annotations ==========
    /// Test identifier for metrics filtering
    /// Example: edgion.io/metrics-test-key: "lb-test-001"
    pub const METRICS_TEST_KEY: &str = "edgion.io/metrics-test-key";
    /// Test type for metrics collection (lb/retry/latency)
    /// Example: edgion.io/metrics-test-type: "lb"
    pub const METRICS_TEST_TYPE: &str = "edgion.io/metrics-test-type";

    /// Reference an EdgionStreamPlugins resource for TCP-level connection filtering.
    /// Value format: "namespace/name" pointing to an EdgionStreamPlugins resource.
    pub const EDGION_STREAM_PLUGINS: &str = "edgion.io/edgion-stream-plugins";

    /// Per-Gateway override for SNI-Host mismatch detection (421 Misdirected Request).
    /// When global `requireSniHostMatch` is false, set this to "true" on a Gateway
    /// to enable listener-level 421 detection for that Gateway's HTTPS listeners.
    pub const REQUIRE_SNI_HOST_MATCH: &str = "edgion.io/require-sni-host-match";

    /// Prefix for operator-defined log labels on resources whose handshake layer
    /// has no plugin chain and no `logging()` stage to hook an `AccessLogExtern`
    /// declaration onto. Any `metadata.annotations` or `metadata.labels` entry
    /// whose key starts with this prefix contributes `<suffix> = <value>` to the
    /// resource's `rsc.labels` in the TLS log.
    /// Example: `edgion.io/log-label.tenant: internal` -> label `tenant=internal`.
    /// Supported on EdgionTls and TLSRoute. HTTPRoute/GRPCRoute log projection is
    /// declared via `AccessLogExtern` instead (see `resources::edgion_plugins`).
    pub const LOG_LABEL_PREFIX: &str = "edgion.io/log-label.";

    /// Reference EdgionPlugins resources to run for every request on this Gateway
    /// (L7, before route-level plugins). Value: comma/space-separated "namespace/name"
    /// list (bare names use the Gateway's namespace).
    pub const EDGION_PLUGINS: &str = "edgion.io/edgion-plugins";

    /// Per-Service DNS resolution mode for a `ServiceExternalName` backend.
    /// Value: `controller` (default) | `gateway` | `per-request`.
    /// - `controller`: use the controller-resolved EndpointSlice when it has a
    ///   ready backend, else the gateway bridges via self-resolution + cache.
    /// - `gateway`: the gateway resolves via the effective resolver + 5s cache
    ///   (authoritative; never consults a slice).
    /// - `per-request`: the gateway resolves on every request, no cache.
    ///
    /// Set on a Service. Invalid values fall back to `controller`.
    pub const DNS_RESOLUTION_MODE: &str = "edgion.io/dns-resolution-mode";

    /// Inline standard-DNS resolver override. Value: a comma/space-separated
    /// server list (`ip[:port]`). Set on a Service (spans both planes) or a
    /// Gateway (data-plane only). Mutually exclusive with `DNS_RESOLVER_REF`.
    pub const DNS_RESOLVER: &str = "edgion.io/dns-resolver";

    /// httpdns LinkSys resolver override. Value: `namespace/name` (explicit
    /// namespace required; bare names are rejected). Set on a Service (spans both
    /// planes) or a Gateway (data-plane only). Mutually exclusive with
    /// `DNS_RESOLVER`.
    pub const DNS_RESOLVER_REF: &str = "edgion.io/dns-resolver-ref";

    /// Controller-written freshness stamp on a synthetic EndpointSlice produced by
    /// the control-plane DNS resolver loop. Value: an RFC3339 timestamp of the
    /// last resolution that changed the endpoint set. NOT operator-set.
    pub const DNS_RESOLVED_AT: &str = "edgion.io/dns-resolved-at";

    /// Parse the `edgion.io/edgion-plugins` (or stream) annotation value into an ordered,
    /// de-duplicated list of "namespace/name" store keys. Bare names (no `/`) are expanded
    /// with `default_ns`. Blank entries are skipped.
    pub fn parse_plugin_ref_list(value: &str, default_ns: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for tok in value.split([',', ' ', '\t', '\n']) {
            let tok = tok.trim();
            if tok.is_empty() {
                continue;
            }
            let key = if tok.contains('/') {
                tok.to_string()
            } else {
                format!("{}/{}", default_ns, tok)
            };
            if !out.contains(&key) {
                out.push(key);
            }
        }
        out
    }

    #[cfg(test)]
    #[path = "../../../../tests/unit/constants/annotations_test/edgion/plugin_ref_list_tests.rs"]
    mod plugin_ref_list_tests;
}
