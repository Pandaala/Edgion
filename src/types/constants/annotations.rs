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

/// Standard Kubernetes annotations
pub mod k8s {
    /// Last applied configuration (used by kubectl apply)
    pub const LAST_APPLIED_CONFIG: &str = "kubectl.kubernetes.io/last-applied-configuration";
}
