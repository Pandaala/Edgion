//! DataSender trait for external system integration
//!
//! Implementations:
//! - [x] LocalFileWriter - file-based with rotation
//! - [x] Elasticsearch sender (bulk indexing)
//! - [x] Kafka async producer
//! - [x] OTLP gRPC exporter (via opentelemetry-otlp)
//! - [ ] FailedCache (spill-on-outage + replay-on-recovery) — planned, not yet
//!   implemented.

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

    /// Whether this sender can currently deliver successfully.
    ///
    /// Contract (all implementations MUST follow this):
    /// - Reflects the most recent delivery outcome or health probe — it is a
    ///   *live* signal, never a constant frozen at `init` time.
    /// - Must turn `false` when the sink knows delivery itself is broken: a
    ///   write/flush failure, a dropped connection, or a dead background worker.
    /// - Two cases intentionally do NOT flip health: transient queue saturation
    ///   is backpressure (a bounded buffer is full), so a sink may stay `true`
    ///   and drop with a metric; and a fire-and-forget transport that cannot
    ///   observe delivery locally (e.g. OTLP export) is driven by a background
    ///   probe rather than the send path.
    /// - Must return to `true` automatically once recovery is observed, driven
    ///   by a later successful delivery or a background probe.
    /// - "Best-effort recent state": bounded staleness (probe interval / TTL) is
    ///   allowed; permanent untruth is not.
    ///
    /// `AccessLogger::send` dispatches to the first sender whose `healthy()` is
    /// `true` (registration order = priority), so an inaccurate value silently
    /// breaks the degradation fallback. On every `healthy <-> unhealthy`
    /// transition an implementation should emit one rate-limited structured log
    /// (and update a metric where one exists).
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
