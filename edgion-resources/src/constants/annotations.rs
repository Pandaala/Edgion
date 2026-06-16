//! Gateway API and Edgion annotation constants

/// Edgion-specific annotations
pub mod edgion {
    /// Enable HTTP/2 support on the listener
    pub const ENABLE_HTTP2: &str = "edgion.io/enable-http2";
    /// Enable automatic HTTP to HTTPS redirect
    pub const HTTP_TO_HTTPS_REDIRECT: &str = "edgion.io/http-to-https-redirect";
    /// Target port for HTTPS redirect
    pub const HTTPS_REDIRECT_PORT: &str = "edgion.io/https-redirect-port";
    /// Expose mTLS client certificate info to plugin layer via TLS digest extension
    pub const EXPOSE_CLIENT_CERT: &str = "edgion.io/expose-client-cert";
    /// Active health check configuration for backend endpoints.
    /// Value format: YAML string.
    pub const HEALTH_CHECK: &str = "edgion.io/health-check";
    /// Circuit breaker configuration for backend endpoints.
    /// Value format: YAML string.
    pub const CIRCUIT_BREAKER: &str = "edgion.io/circuit-breaker";
    /// Outlier detection (passive health check) configuration for backend
    /// endpoints. Canonical key; supersedes the deprecated `CIRCUIT_BREAKER`
    /// alias, which is still parsed for backward compatibility.
    /// Value format: YAML string.
    pub const OUTLIER_DETECTION: &str = "edgion.io/outlier-detection";

    /// Per-service load-balancing panic threshold (percent, 0-100).
    /// When the fraction of healthy backends drops below this percent, the
    /// gateway ignores health filtering and load-balances across all backends.
    /// Value: integer string. 0 (default) disables panic mode.
    pub const PANIC_THRESHOLD: &str = "edgion.io/panic-threshold";

    /// Proxy Protocol version to send to upstream.
    /// Value: "v2" to enable PP2.
    pub const PROXY_PROTOCOL: &str = "edgion.io/proxy-protocol";

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
}
