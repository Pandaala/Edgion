//! HTTP header constants

/// Proxy-related headers for forwarding client information
pub mod proxy {
    /// Client's original IP address (may contain multiple IPs)
    pub const X_FORWARDED_FOR: &str = "X-Forwarded-For";
    /// Client's real IP address (single IP)
    pub const X_REAL_IP: &str = "X-Real-IP";
    /// Original protocol (http or https)
    pub const X_FORWARDED_PROTO: &str = "X-Forwarded-Proto";
    /// Original host header
    pub const X_FORWARDED_HOST: &str = "X-Forwarded-Host";
    /// Original port
    pub const X_FORWARDED_PORT: &str = "X-Forwarded-Port";
}

/// WebSocket and HTTP upgrade headers
pub mod upgrade {
    /// Upgrade header for protocol switching
    pub const UPGRADE: &str = "upgrade";
    /// Connection header
    pub const CONNECTION: &str = "connection";
    /// WebSocket protocol value
    pub const WEBSOCKET: &str = "websocket";
}

/// CORS (Cross-Origin Resource Sharing) headers
pub mod cors {
    /// Request origin
    pub const ORIGIN: &str = "origin";
    /// Preflight request method
    pub const ACCESS_CONTROL_REQUEST_METHOD: &str = "access-control-request-method";
    /// Preflight request headers
    pub const ACCESS_CONTROL_REQUEST_HEADERS: &str = "access-control-request-headers";
    /// Allowed origin in response
    pub const ACCESS_CONTROL_ALLOW_ORIGIN: &str = "access-control-allow-origin";
    /// Allowed methods in response
    pub const ACCESS_CONTROL_ALLOW_METHODS: &str = "access-control-allow-methods";
    /// Allowed headers in response
    pub const ACCESS_CONTROL_ALLOW_HEADERS: &str = "access-control-allow-headers";
    /// Max age for preflight cache
    pub const ACCESS_CONTROL_MAX_AGE: &str = "access-control-max-age";
    /// Allow credentials
    pub const ACCESS_CONTROL_ALLOW_CREDENTIALS: &str = "access-control-allow-credentials";
    /// Expose headers to client
    pub const ACCESS_CONTROL_EXPOSE_HEADERS: &str = "access-control-expose-headers";
}
