pub(crate) mod apply;
#[cfg(feature = "otlp")]
pub mod client;
#[cfg(not(feature = "otlp"))]
#[path = "client_disabled.rs"]
pub mod client;
pub mod log_sender;
#[cfg(feature = "otlp")]
mod transport;
pub(crate) use apply::OtlpApply;
pub use client::OtlpLinkClient;
pub use log_sender::OtlpLogSender;
