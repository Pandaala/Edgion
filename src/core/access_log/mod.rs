//! Access log module
//!
//! Provides access logging functionality with pluggable sinks.

mod entry;
mod logger;

pub use entry::AccessLogEntry;
pub use logger::{AccessLogger, AccessLoggerHandle, DEFAULT_QUEUE_CAPACITY};

