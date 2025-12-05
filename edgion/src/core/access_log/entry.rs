//! Access log entry definition
//!
//! Defines the structure for access log entries with references to context data.

use crate::types::{EdgionHttpContext, EdgionStatus};

/// Access log entry holding references to request context
///
/// Uses references to avoid copying data. The entry is formatted to string
/// before being sent to the log sink.
pub struct AccessLogEntry<'a> {
    /// Trace ID for distributed tracing
    pub x_trace_id: Option<&'a str>,
    /// Request ID
    pub request_id: Option<&'a str>,
    /// Request hostname
    pub hostname: &'a str,
    /// Request path
    pub path: &'a str,
    /// Response status code
    pub status: u16,
    /// Error codes collected during processing
    pub error_codes: &'a [EdgionStatus],
    /// Route namespace (if matched)
    pub route_ns: Option<&'a str>,
    /// Route name (if matched)
    pub route_name: Option<&'a str>,
    /// Upstream service name
    pub upstream_name: Option<&'a str>,
    /// Upstream service namespace
    pub upstream_ns: Option<&'a str>,
    /// Upstream peer address
    pub upstream_peer: Option<&'a str>,
    /// Request latency in milliseconds
    pub latency_ms: u64,
    /// Timestamp (epoch millis)
    pub timestamp: i64,
}

impl<'a> AccessLogEntry<'a> {
    /// Create an AccessLogEntry from EdgionHttpContext
    pub fn from_context(ctx: &'a EdgionHttpContext, latency_ms: u64) -> Self {
        Self {
            x_trace_id: ctx.x_trace_id.as_deref(),
            request_id: ctx.request_id.as_deref(),
            hostname: &ctx.request_info.hostname,
            path: &ctx.request_info.path,
            status: ctx.request_info.status,
            error_codes: &ctx.error_codes,
            route_ns: ctx.matched_info.as_ref().map(|m| m.rns.as_str()),
            route_name: ctx.matched_info.as_ref().map(|m| m.rn.as_str()),
            upstream_name: ctx.upstream_info.as_ref().map(|u| u.name.as_str()),
            upstream_ns: ctx.upstream_info.as_ref().map(|u| u.namespace.as_str()),
            upstream_peer: ctx.upstream_info.as_ref().map(|u| u.peer.as_str()),
            latency_ms,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Format the entry as JSON string
    pub fn to_json(&self) -> String {
        let error_codes_str = if self.error_codes.is_empty() {
            "[]".to_string()
        } else {
            let codes: Vec<String> = self.error_codes.iter()
                .map(|e| format!("\"{:?}\"", e))
                .collect();
            format!("[{}]", codes.join(","))
        };

        format!(
            r#"{{"ts":{},"trace_id":"{}","req_id":"{}","host":"{}","path":"{}","status":{},"errors":{},"route":"{}/{}","upstream":"{}/{}/{}","latency_ms":{}}}"#,
            self.timestamp,
            self.x_trace_id.unwrap_or("-"),
            self.request_id.unwrap_or("-"),
            self.hostname,
            self.path,
            self.status,
            error_codes_str,
            self.route_ns.unwrap_or("-"),
            self.route_name.unwrap_or("-"),
            self.upstream_ns.unwrap_or("-"),
            self.upstream_name.unwrap_or("-"),
            self.upstream_peer.unwrap_or("-"),
            self.latency_ms,
        )
    }

    /// Format the entry as combined log format (like nginx)
    pub fn to_combined(&self) -> String {
        format!(
            r#"{} - - [{}] "{} {}" {} {} "{}" "{}""#,
            self.upstream_peer.unwrap_or("-"),
            chrono::DateTime::from_timestamp_millis(self.timestamp)
                .map(|dt| dt.format("%d/%b/%Y:%H:%M:%S %z").to_string())
                .unwrap_or_else(|| "-".to_string()),
            "GET", // TODO: add method to RequestInfo
            self.path,
            self.status,
            self.latency_ms,
            self.x_trace_id.unwrap_or("-"),
            self.request_id.unwrap_or("-"),
        )
    }
}

