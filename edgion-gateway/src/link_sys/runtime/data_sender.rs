//! DataSender trait for external system integration
//!
//! Implementations:
//! - [x] LocalFileWriter - file-based with rotation
//! - [x] Elasticsearch sender (bulk indexing)
//! - [x] Kafka async producer
//! - [x] OTLP gRPC exporter (via opentelemetry-otlp)
//! - [ ] FailedCache (spill-on-outage + replay-on-recovery) — planned, not yet
//!   implemented; see `link_sys/readme` for the original design.

use anyhow::Result;
use async_trait::async_trait;

/// Pre-serialization metadata snapshot for senders that need typed fields.
///
/// Extracted from the access log entry before JSON serialization so that
/// OTLP (and future) senders can set structured LogRecord fields without
/// re-parsing the JSON string.
///
/// `method` and `path` are owned `String` because `DataSender` requires
/// `Send + 'static` and the entry's references do not outlive the call site.
#[derive(Debug, Clone, Default)]
pub struct LogMeta {
    /// Event timestamp in nanoseconds since Unix epoch.
    pub timestamp_ns: u64,
    /// OTel SeverityNumber (see opentelemetry-proto logs.proto).
    /// 0 = unset, 9 = INFO, 13 = WARN, 17 = ERROR.
    pub severity_number: u32,
    /// HTTP response status code (None for non-HTTP streams).
    pub status_code: Option<u16>,
    /// HTTP request method (e.g. "GET", "POST"). None for non-HTTP streams.
    pub method: Option<String>,
    /// Normalized request path. None for non-HTTP streams.
    pub path: Option<String>,
}

impl LogMeta {
    /// Derive OTel SeverityNumber from HTTP status code.
    pub fn severity_from_status(status: Option<u16>) -> u32 {
        match status {
            Some(s) if s >= 500 => 17, // ERROR
            Some(s) if s >= 400 => 13, // WARN
            Some(_) => 9,              // INFO
            None => 0,                 // UNSPECIFIED
        }
    }
}

/// Trait for sending data to external systems
///
/// Generic over the data type `T` to support different payload types:
/// - `String` for text-based logs (LocalFileWriter, ES, Kafka)
/// - With typed metadata for structured senders (OTLP)
#[async_trait]
pub trait DataSender<T>: Send + Sync
where
    T: Send + 'static,
{
    /// Initialize the sender connection
    async fn init(&mut self) -> Result<()>;

    /// Check if the sender is healthy
    fn healthy(&self) -> bool;

    /// Send data to the external system (takes ownership to avoid copy)
    async fn send(&self, data: T) -> Result<()>;

    /// Send data with pre-serialization metadata.
    ///
    /// The default implementation ignores `_meta` and delegates to `send`.
    /// Senders that benefit from structured metadata (e.g. OTLP) override this.
    async fn send_with_meta(&self, data: T, _meta: &LogMeta) -> Result<()> {
        self.send(data).await
    }

    /// Get the name of this sender (for logging)
    fn name(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_severity_from_status() {
        assert_eq!(LogMeta::severity_from_status(Some(200)), 9);
        assert_eq!(LogMeta::severity_from_status(Some(301)), 9);
        assert_eq!(LogMeta::severity_from_status(Some(400)), 13);
        assert_eq!(LogMeta::severity_from_status(Some(404)), 13);
        assert_eq!(LogMeta::severity_from_status(Some(500)), 17);
        assert_eq!(LogMeta::severity_from_status(Some(503)), 17);
        assert_eq!(LogMeta::severity_from_status(None), 0);
    }
}
