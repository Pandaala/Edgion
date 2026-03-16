//! Gateway API and Edgion annotation constants

/// Edgion-specific annotations
pub mod edgion {
    /// Enable HTTP/2 support on the listener
    pub const ENABLE_HTTP2: &str = "edgion.io/enable-http2";
    /// Backend protocol (tcp, http, grpc, etc.)
    pub const BACKEND_PROTOCOL: &str = "edgion.io/backend-protocol";
    /// Enable automatic HTTP to HTTPS redirect
    pub const HTTP_TO_HTTPS_REDIRECT: &str = "edgion.io/http-to-https-redirect";
    /// Target port for HTTPS redirect
    pub const HTTPS_REDIRECT_PORT: &str = "edgion.io/https-redirect-port";
    /// Expose mTLS client certificate info to plugin layer via TLS digest extension
    pub const EXPOSE_CLIENT_CERT: &str = "edgion.io/expose-client-cert";
    /// Active health check configuration for backend endpoints.
    /// Value format: YAML string.
    pub const HEALTH_CHECK: &str = "edgion.io/health-check";

    /// Proxy Protocol version to send to upstream.
    /// Value: "v2" to enable PP2.
    pub const PROXY_PROTOCOL: &str = "edgion.io/proxy-protocol";

    /// Whether to use TLS when connecting to upstream.
    /// Value: "true" or "false" (default: "false").
    pub const UPSTREAM_TLS: &str = "edgion.io/upstream-tls";

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
}

/// Standard Kubernetes annotations
pub mod k8s {
    /// Last applied configuration (used by kubectl apply)
    pub const LAST_APPLIED_CONFIG: &str = "kubectl.kubernetes.io/last-applied-configuration";
}
