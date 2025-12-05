//! Access log entry definition

use crate::types::{EdgionHttpContext, EdgionStatus, RequestInfo, UpstreamInfo};

/// Access log entry with only essential fields
pub struct AccessLogEntry<'a> {
    pub x_trace_id: Option<&'a str>,
    pub request_info: &'a RequestInfo,
    pub error_codes: &'a [EdgionStatus],
    pub upstream_info: Option<&'a UpstreamInfo>,
    pub latency_ms: u64,
    pub timestamp: i64,
}

impl<'a> AccessLogEntry<'a> {
    pub fn from_context(ctx: &'a EdgionHttpContext, latency_ms: u64) -> Self {
        Self {
            x_trace_id: ctx.x_trace_id.as_deref(),
            request_info: &ctx.request_info,
            error_codes: &ctx.error_codes,
            upstream_info: ctx.upstream_info.as_ref(),
            latency_ms,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }

    pub fn to_json(&self) -> String {
        let error_codes_str = if self.error_codes.is_empty() {
            "[]".to_string()
        } else {
            let codes: Vec<String> = self.error_codes.iter()
                .map(|e| format!("\"{:?}\"", e))
                .collect();
            format!("[{}]", codes.join(","))
        };
        
        let (upstream_ns, upstream_name, upstream_peer) = self.upstream_info
            .map(|u| (u.namespace.as_str(), u.name.as_str(), u.peer.as_str()))
            .unwrap_or(("-", "-", "-"));

        format!(
            r#"{{"ts":{},"trace_id":"{}","host":"{}","path":"{}","status":{},"errors":{},"upstream":"{}/{}/{}","latency_ms":{}}}"#,
            self.timestamp,
            self.x_trace_id.unwrap_or("-"),
            self.request_info.hostname,
            self.request_info.path,
            self.request_info.status,
            error_codes_str,
            upstream_ns,
            upstream_name,
            upstream_peer,
            self.latency_ms,
        )
    }

    pub fn to_combined(&self) -> String {
        let upstream_peer = self.upstream_info
            .map(|u| u.peer.as_str())
            .unwrap_or("-");
            
        format!(
            r#"{} - - [{}] "{} {}" {} {} "{}""#,
            upstream_peer,
            chrono::DateTime::from_timestamp_millis(self.timestamp)
                .map(|dt| dt.format("%d/%b/%Y:%H:%M:%S %z").to_string())
                .unwrap_or_else(|| "-".to_string()),
            "GET",
            self.request_info.path,
            self.request_info.status,
            self.latency_ms,
            self.x_trace_id.unwrap_or("-"),
        )
    }
}

