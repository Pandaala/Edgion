//! Filter system for request/response processing

pub mod extended;
pub mod filter_log;
pub mod runtime;
pub mod session_adapter;
pub mod standard;
pub mod traits;

pub use filter_log::FilterLog;
pub use runtime::FilterRuntime;
pub use standard::RequestHeaderModifierFilter;
pub use traits::{Filter, FilterSession, FilterSessionError, FilterSessionResult};

#[cfg(test)]
pub use traits::MockFilterSession;
