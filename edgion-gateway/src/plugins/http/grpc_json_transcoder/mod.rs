//! Descriptor-backed REST to native gRPC exchange ownership.
mod budget;
pub mod log_consts;
mod plugin;
mod response;
pub(crate) mod transport;
pub use plugin::GrpcJsonTranscoder;
pub(crate) use response::status_evidence;
pub(crate) use response::GrpcResponseProcessor;
pub use transport::GrpcTranscodingPlan;
pub(crate) use transport::{failure, prepare_attempt, require_h2, validate_request};
