//! Filter traits for different plugin stages

pub mod ai_session;
pub mod header_ext;
pub mod request_body_stream;
pub mod request_filter;
pub mod session;
pub mod streaming_response_processor;
#[cfg(test)]
#[path = "../../../../tests/unit/plugins/runtime/traits/mod_test/test_helpers.rs"]
pub mod test_helpers;
pub mod upstream_response;
pub mod upstream_response_body_filter;
pub mod upstream_response_filter;
pub mod upstream_response_trailer;
pub mod waf_session;

pub use ai_session::AiSessionCapability;
pub use header_ext::PluginSessionHeaderExt;
pub use request_body_stream::{ChunkVerdict, RequestBodyStreamHandler};
pub use request_filter::RequestFilter;
pub use session::{DyeOutcome, PluginSession, PluginSessionError, PluginSessionResult};
pub use streaming_response_processor::{StreamingResponseEvent, StreamingResponseProcessor};
pub use upstream_response::UpstreamResponse;
pub use upstream_response_body_filter::UpstreamResponseBodyFilter;
pub use upstream_response_filter::UpstreamResponseFilter;
pub use upstream_response_trailer::{UpstreamResponseTrailer, UpstreamTrailerResult};

pub use waf_session::WafSessionCapability;
