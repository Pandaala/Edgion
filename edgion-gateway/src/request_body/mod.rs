//! Request-body capture, storage, inspection, mutation, and upstream replay.
//!
//! This is a gateway-level subsystem shared by the Pingora transport path,
//! request context, and body-aware plugins. It caches proxied request bodies
//! only; proxied upstream responses remain streaming-only.

pub mod config;
mod state;
#[cfg(feature = "body-buffering")]
mod storage;

pub use config::{ResolvedRequestBodyConfig, DEFAULT_MEMORY_BUFFER_BYTES};
pub use state::RequestBodyState;
#[cfg(feature = "body-buffering")]
pub use storage::{BodyStorage, BodyStorageView};
