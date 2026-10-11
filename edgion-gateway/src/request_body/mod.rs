//! Request-body capture, storage, inspection, mutation, and upstream replay.
//!
//! This is a gateway-level subsystem shared by the Pingora transport path,
//! request context, and body-aware plugins. It caches proxied request bodies
//! only; proxied upstream responses remain streaming-only.

mod body_scanners;
pub mod config;
mod contracts;
pub(crate) mod observer;
mod operation;
mod state;
mod storage;

pub(crate) use body_scanners::{HmacSha256Scanner, Md5Scanner};
pub use config::{ResolvedRequestBodyConfig, DEFAULT_MEMORY_BUFFER_BYTES};
pub use contracts::{BodyReadError, BodyReadResult, RequestBodyChunkConsumer, RequestBodySnapshotReader};
pub use operation::{
    BodyOperationError, BodyScanResult, BodyTextOperation, BodyTextResult, BoundedTextResult, LossyTextScanner,
    RawBodyScanner, ScanControl, TextBodyScanner, MAX_BODY_SEGMENTS,
};
pub use state::RequestBodyState;
pub(crate) use storage::run_blocking_scan;
pub use storage::{BodyStorage, BodyStorageView};
#[cfg(test)]
pub(crate) use storage::{StorageIoOperation, STORAGE_IO_TEST_HOOK};

/// SHA-256 of the transport-proven empty request-body version.
pub(crate) const EMPTY_SHA256: [u8; 32] = [
    0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f, 0xb9, 0x24, 0x27, 0xae, 0x41,
    0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b, 0x78, 0x52, 0xb8, 0x55,
];

/// Whether a captured request body was being replayed to the upstream on this
/// session. Error handling uses this to attribute a gateway-local replay
/// failure in the access log (`EdgionErrorCode::BodyReplayFailed`) — the final
/// error alone only shows an unactionable `InternalError`.
pub fn request_body_replay_active(session: &pingora_proxy::Session) -> bool {
    session.as_ref().request_body_buffer_replaying()
}

#[cfg(test)]
#[path = "../../tests/unit/request_body/mod_test.rs"]
mod tests;
