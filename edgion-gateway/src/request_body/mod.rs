//! Request-body capture, storage, inspection, mutation, and upstream replay.
//!
//! This is a gateway-level subsystem shared by the Pingora transport path,
//! request context, and body-aware plugins. It caches proxied request bodies
//! only; proxied upstream responses remain streaming-only.

mod body_scanners;
pub mod config;
pub(crate) mod observer;
mod operation;
mod state;
mod storage;

pub(crate) use body_scanners::{
    HmacSha256Scanner, JsonDuplicatePolicy, JsonPathScanner, JsonScanValue, JsonSelectPlan, JsonSelectScanner,
    Md5Scanner, StreamingRegexScanner,
};
pub use config::{ResolvedRequestBodyConfig, DEFAULT_MEMORY_BUFFER_BYTES};
pub use operation::{
    BodyOperationError, BodyScanResult, BodyTextOperation, BodyTextResult, LossyTextScanner, RawBodyScanner,
    ScanControl, TextBodyScanner, MAX_BODY_SEGMENTS,
};
pub use state::RequestBodyState;
pub use storage::{BodyStorage, BodyStorageView};

/// Whether a captured request body was being replayed to the upstream on this
/// session. Error handling uses this to attribute a gateway-local replay
/// failure in the access log (`EdgionStatus::BodyReplayFailed`) — the final
/// error alone only shows an unactionable `InternalError`.
pub fn request_body_replay_active(session: &pingora_proxy::Session) -> bool {
    session.as_ref().request_body_buffer_replaying()
}
