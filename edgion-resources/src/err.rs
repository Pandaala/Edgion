use pingora_core::Error as PingoraError;

/// Watch error constants
pub const WATCH_ERR_VERSION_UNEXPECTED: &str = "VersionUnexpect";
pub const WATCH_ERR_TOO_OLD_VERSION: &str = "TooOldVersion";
pub const WATCH_ERR_EVENTS_LOST: &str = "EventsLost";
pub const WATCH_ERR_NOT_READY: &str = "NotReady";
pub const WATCH_ERR_SERVER_ID_MISMATCH: &str = "ServerIdMismatch";
pub const WATCH_ERR_SERVER_RELOAD: &str = "ServerReload";

#[derive(Debug, thiserror::Error)]
pub enum EdError {
    #[error("Pingora error: {0}")]
    Pingora(#[from] PingoraError),

    #[error("Route not found")]
    RouteNotFound(),

    #[error("Invalid config: {0}")]
    InvalidConfig(String),

    #[error("Internal error: {0}")]
    InternalError(String),

    #[error("Service unavailable: {0}")]
    ServiceUnavailable(String),

    #[error("Upstream error {0}")]
    UpstreamError(String),

    #[error("Sni not match_engine: {0}")]
    SniNotMatch(String),

    #[error("Route match error: {0}")]
    RouteMatchError(String),

    #[error("No backend available")]
    BackendNotFound(),

    #[error("Inconsistent backend weight configuration")]
    InconsistentWeight(),

    #[error("Invalid gRPC path format: {0}, expected /<service>/<method>")]
    InvalidGrpcPath(String),

    #[error("Plugin terminated the request")]
    PluginTerminated(),

    #[error("HTTP/2 required for gRPC")]
    Http2Required,

    #[error("Cross-namespace reference denied: {target_namespace}/{target_name} ({reason})")]
    RefDenied {
        target_namespace: String,
        target_name: String,
        reason: String,
    },
}
