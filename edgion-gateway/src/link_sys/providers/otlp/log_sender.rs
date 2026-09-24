//! Key-only OTLP consumer. Connection, credentials and SDK ownership live in LinkSys.
use std::time::{Duration, SystemTime};

use anyhow::Result;
use async_trait::async_trait;
use edgion_resources::output::LinkSysRef;
use edgion_resources::sensitive::apply_sensitive;
use opentelemetry::logs::{AnyValue, LogRecord, Logger, LoggerProvider, Severity};
use opentelemetry_sdk::logs::SdkLoggerProvider;

use crate::link_sys::runtime::store::get_otlp_client;
use crate::link_sys::{DataSender, LogMeta};

pub struct OtlpLogSender {
    key: String,
}

impl OtlpLogSender {
    pub fn new(reference: LinkSysRef) -> Result<Self> {
        reference.validate().map_err(anyhow::Error::msg)?;
        Ok(Self { key: reference.key() })
    }
}

#[async_trait]
impl DataSender<String> for OtlpLogSender {
    async fn init(&mut self) -> Result<()> {
        Ok(())
    }
    fn healthy(&self) -> bool {
        get_otlp_client(&self.key).is_some_and(|client| client.healthy())
    }
    async fn send(&self, data: String) -> Result<()> {
        let meta = LogMeta {
            timestamp_ns: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64,
            ..Default::default()
        };
        self.send_with_meta(data, &meta).await
    }
    async fn send_with_meta(&self, data: String, meta: &LogMeta<'_>) -> Result<()> {
        get_otlp_client(&self.key)
            .ok_or_else(|| anyhow::anyhow!("otlp_unavailable"))?
            .send_with_meta(data, meta)
    }
    fn name(&self) -> &str {
        &self.key
    }
}

pub(super) fn emit_record(provider: &SdkLoggerProvider, data: String, meta: &LogMeta<'_>) {
    let logger = provider.logger("edgion-gateway");
    let mut record = logger.create_log_record();
    record.set_body(AnyValue::String(data.into()));
    attach_request_trace_id(&mut record, meta.trace_id);
    record.set_timestamp(SystemTime::UNIX_EPOCH + Duration::from_nanos(meta.timestamp_ns));
    record.set_observed_timestamp(SystemTime::now());
    if let Some(severity) = numeric_to_severity(meta.severity_number) {
        record.set_severity_number(severity);
    }
    if let Some(status) = meta.status_code {
        record.add_attribute("http.response.status_code", status as i64);
    }
    if let Some(method) = meta.method {
        record.add_attribute("http.request.method", method.to_owned());
    }
    if let Some(path) = meta.path {
        record.add_attribute("url.path", render_sensitive_path(path));
    }
    logger.emit(record);
}

fn attach_request_trace_id(record: &mut impl LogRecord, trace_id: Option<[u8; 16]>) {
    if let Some(trace_id) = trace_id {
        // The SDK requires a SpanId; INVALID means no span association.
        // Never label the remote parent as an exported gateway span.
        record.set_trace_context(
            opentelemetry::TraceId::from_bytes(trace_id),
            opentelemetry::SpanId::INVALID,
            None,
        );
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

#[inline]
fn render_sensitive_path(path: &str) -> String {
    apply_sensitive(path, str::to_owned)
}

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/otlp/log_sender_test.rs"]
mod tests;
