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
}

/// Standard Kubernetes annotations
pub mod k8s {
    /// Last applied configuration (used by kubectl apply)
    pub const LAST_APPLIED_CONFIG: &str = "kubectl.kubernetes.io/last-applied-configuration";
}
