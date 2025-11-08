use pingora_core::Error as PingoraError;

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

    #[error("Sni not match: {0}")]
    SniNotMatch(String),
}
