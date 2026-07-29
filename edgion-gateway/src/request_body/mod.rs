//! Request-body capture, storage, inspection, mutation, and upstream replay.
//!
//! This is a gateway-level subsystem shared by the Pingora transport path,
//! request context, and body-aware plugins. It caches proxied request bodies
//! only; proxied upstream responses remain streaming-only.

pub mod config;
mod operation;
mod state;
#[cfg(feature = "body-buffering")]
mod storage;

pub use config::{ResolvedRequestBodyConfig, DEFAULT_MEMORY_BUFFER_BYTES};
pub use operation::{
    BodyOperationError, BodyScanResult, BodyTextOperation, BodyTextResult, LossyTextScanner, RawBodyScanner,
    ScanControl, TextBodyScanner, MAX_BODY_SEGMENTS,
};
pub use state::RequestBodyState;
#[cfg(feature = "body-buffering")]
pub use storage::{BodyStorage, BodyStorageView};

/// Whether a captured request body was being replayed to the upstream on this
/// session. Error handling uses this to attribute a gateway-local replay
/// failure in the access log (`EdgionStatus::BodyReplayFailed`) — the final
/// error alone only shows an unactionable `InternalError`.
#[cfg(feature = "body-buffering")]
pub fn request_body_replay_active(session: &pingora_proxy::Session) -> bool {
    session.as_ref().request_body_buffer_replaying()
}

/// Stock flavor: no replay machinery exists, so replay is never active.
#[cfg(all(feature = "upstream-pingora", not(feature = "body-buffering")))]
pub fn request_body_replay_active(_session: &pingora_proxy::Session) -> bool {
    false
}
