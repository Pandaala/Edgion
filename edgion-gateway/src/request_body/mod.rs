//! Request-body capture, storage, inspection, mutation, and upstream replay.
//!
//! This is a gateway-level subsystem shared by the Pingora transport path,
//! request context, and body-aware plugins. It caches proxied request bodies
//! only; proxied upstream responses remain streaming-only.

mod storage;

pub use storage::{BodyStorage, BodyStorageView, DEFAULT_MEMORY_BUFFER_BYTES};
