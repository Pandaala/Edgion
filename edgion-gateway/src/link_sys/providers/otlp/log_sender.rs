use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use async_trait::async_trait;
use opentelemetry::logs::{AnyValue, LogRecord, Logger, LoggerProvider, Severity};
use opentelemetry_otlp::{LogExporter, WithExportConfig};
use opentelemetry_sdk::logs::SdkLoggerProvider;

use crate::link_sys::{DataSender, LogMeta};
use edgion_resources::output::{OtlpOutputConfig, OtlpProtocol};

pub struct OtlpLogSender {
    config: OtlpOutputConfig,
    provider: OnceLock<SdkLoggerProvider>,
    healthy: AtomicBool,
}

impl OtlpLogSender {
    pub fn new(config: OtlpOutputConfig) -> Self {
        Self {
            config,
            provider: OnceLock::new(),
            healthy: AtomicBool::new(false),
        }
    }
}

/// Map a numeric OTel SeverityNumber (0-24) to the SDK enum.
///
/// Returns None when severity is 0 (unset) — caller skips `set_severity_number`.
///
/// NOTE: This groups Trace/Debug/Info/Warn/Error/Fatal into their base variants
/// (e.g. `n=2` maps to `Severity::Trace`, not `Severity::Trace2`). HTTP access
/// log callers only produce 0/9/13/17 (from `severity_from_status`), so no
/// precision is lost in practice. Future callers that emit finer-grained numbers
/// should extend this to the full 24-variant map.
pub(crate) fn numeric_to_severity(n: u32) -> Option<Severity> {
    match n {
        1..=4 => Some(Severity::Trace),
        5..=8 => Some(Severity::Debug),
        9..=12 => Some(Severity::Info),
        13..=16 => Some(Severity::Warn),
        17..=20 => Some(Severity::Error),
        21..=24 => Some(Severity::Fatal),
        _ => None,
    }
}

#[async_trait]
impl DataSender<String> for OtlpLogSender {
    async fn init(&mut self) -> Result<()> {
        // Only gRPC is supported in v1 due to tonic version constraints.
        // The workspace uses tonic 0.12; opentelemetry-otlp pulls tonic 0.14
        // internally. Constructing tonic 0.14 types (e.g. MetadataMap) from
        // gateway code (compiled against tonic 0.12) is a compile-time type
        // mismatch, so HttpProto and custom headers are deferred.
        if self.config.protocol != OtlpProtocol::Grpc {
            anyhow::bail!(
                "OTLP protocol {:?} is not yet supported; only Grpc is implemented in v1",
                self.config.protocol
            );
        }
        if self.config.headers.is_some() {
            tracing::warn!(
                endpoint = %self.config.endpoint,
                "OTLP custom headers are not supported in v1 (tonic version constraint); headers ignored"
            );
        }

        let mut builder = LogExporter::builder().with_tonic().with_endpoint(&self.config.endpoint);
        if let Some(ms) = self.config.timeout_ms {
            builder = builder.with_timeout(Duration::from_millis(ms));
        }
        let exporter = builder
            .build()
            .map_err(|e| anyhow::anyhow!("OTLP exporter build failed for '{}': {}", self.config.endpoint, e))?;

        // Pass the LogExporter directly — the SDK wraps it in a BatchLogProcessor internally.
        let provider = SdkLoggerProvider::builder().with_batch_exporter(exporter).build();

        self.provider.set(provider).ok();
        self.healthy.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    async fn send(&self, data: String) -> Result<()> {
        // Fallback path: no typed meta available (TCP/UDP/TLS log paths that
        // still call send() rather than send_with_meta()).
        let meta = LogMeta {
            timestamp_ns: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64,
            ..Default::default()
        };
        self.send_with_meta(data, &meta).await
    }

    async fn send_with_meta(&self, data: String, meta: &LogMeta) -> Result<()> {
        let provider = self
            .provider
            .get()
            .ok_or_else(|| anyhow::anyhow!("OTLP sender '{}' not initialized", self.config.endpoint))?;

        // provider.logger() is cheap (returns a reference clone).
        let logger = provider.logger("edgion-gateway");

        let ts = SystemTime::UNIX_EPOCH + Duration::from_nanos(meta.timestamp_ns);
        let mut record = logger.create_log_record();
        record.set_body(AnyValue::String(data.into()));
        record.set_timestamp(ts);
        record.set_observed_timestamp(SystemTime::now());
        if let Some(sev) = numeric_to_severity(meta.severity_number) {
            record.set_severity_number(sev);
        }

        if let Some(status) = meta.status_code {
            record.add_attribute("http.response.status_code", status as i64);
        }
        if let Some(ref method) = meta.method {
            // Use .clone() — non-'static &str is not Into<AnyValue> in opentelemetry 0.31.
            record.add_attribute("http.request.method", method.clone());
        }
        if let Some(ref path) = meta.path {
            record.add_attribute("url.path", path.clone());
        }

        logger.emit(record);
        Ok(())
    }

    fn name(&self) -> &str {
        &self.config.endpoint
    }
}

// SdkLoggerProvider's Drop impl calls shutdown(), which blocks until the
// BatchLogProcessor has flushed pending records. When OtlpLogSender is dropped
// during graceful shutdown (via AccessLogger drop), the batch queue is flushed
// automatically.
//
// CAUTION: shutdown() drives the gRPC flush via `futures_executor::block_on`
// while holding an internal lock. On a Tokio current-thread runtime (e.g.
// #[tokio::test] without multi_thread), calling init() and then dropping the
// sender on the runtime thread can deadlock because block_on cannot schedule the
// Tokio IO future. Tests that call init() must use a multi-threaded runtime or
// call shutdown() from a separate thread.

#[cfg(test)]
mod tests {
    use super::*;

    fn make_grpc_config() -> OtlpOutputConfig {
        OtlpOutputConfig {
            endpoint: "http://localhost:4317".to_string(),
            protocol: OtlpProtocol::Grpc,
            headers: None,
            timeout_ms: None,
        }
    }

    #[test]
    fn test_otlp_log_sender_name() {
        let sender = OtlpLogSender::new(make_grpc_config());
        assert_eq!(sender.name(), "http://localhost:4317");
    }

    #[test]
    fn test_otlp_log_sender_healthy_before_init() {
        let sender = OtlpLogSender::new(make_grpc_config());
        assert!(!sender.healthy());
    }

    #[test]
    fn test_severity_mapping() {
        assert_eq!(numeric_to_severity(0), None);
        assert_eq!(numeric_to_severity(9), Some(Severity::Info));
        assert_eq!(numeric_to_severity(13), Some(Severity::Warn));
        assert_eq!(numeric_to_severity(17), Some(Severity::Error));
        assert_eq!(numeric_to_severity(24), Some(Severity::Fatal));
        assert_eq!(numeric_to_severity(25), None);
    }

    #[tokio::test]
    async fn test_init_http_proto_returns_error() {
        let mut sender = OtlpLogSender::new(OtlpOutputConfig {
            endpoint: "http://localhost:4318".to_string(),
            protocol: OtlpProtocol::HttpProto,
            headers: None,
            timeout_ms: None,
        });
        let err = sender.init().await.unwrap_err();
        assert!(err.to_string().contains("not yet supported"), "unexpected error: {err}");
        assert!(!sender.healthy(), "sender must not be marked healthy after failed init");
    }
}
