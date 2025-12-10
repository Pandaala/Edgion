//! Access log entry definition

use crate::types::{EdgionHttpContext, EdgionStatus, RequestInfo, UpstreamInfo};
use serde::Serialize;

/// Helper function to check if a slice is empty
fn is_empty<T>(slice: &&[T]) -> bool {
    slice.is_empty()
}

/// Access log entry with only essential fields
#[derive(Serialize)]
pub struct AccessLogEntry<'a> {
    #[serde(rename = "ts")]
    pub timestamp: i64,
    
    pub request_info: &'a RequestInfo,
    
    #[serde(skip_serializing_if = "is_empty")]
    pub errors: &'a [EdgionStatus],
    
    pub upstream_info: &'a [UpstreamInfo],
}

impl<'a> AccessLogEntry<'a> {
    pub fn from_context(ctx: &'a EdgionHttpContext) -> Self {
        Self {
            timestamp: chrono::Utc::now().timestamp_millis(),
            request_info: &ctx.request_info,
            errors: &ctx.error_codes,
            upstream_info: &ctx.upstream_info,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|e| {
            tracing::error!("Failed to serialize access log: {}", e);
            "{}".to_string()
        })
    }

    pub fn to_combined(&self, latency_ms: u64) -> String {
        // Use the last upstream_info for the combined log format
        let upstream_peer = self.upstream_info.last()
            .map(|u| {
                let ip = u.ip.as_deref().unwrap_or("-");
                let port = u.port.map(|p| p.to_string()).unwrap_or_else(|| "-".to_string());
                format!("{}:{}", ip, port)
            })
            .unwrap_or_else(|| "-".to_string());
            
        format!(
            r#"{} - - [{}] "{} {}" {} {} "{}""#,
            upstream_peer,
            chrono::DateTime::from_timestamp_millis(self.timestamp)
                .map(|dt| dt.format("%d/%b/%Y:%H:%M:%S %z").to_string())
                .unwrap_or_else(|| "-".to_string()),
            "GET",
            self.request_info.path,
            self.request_info.status,
            latency_ms,
            self.request_info.x_trace_id.as_deref().unwrap_or("-"),
        )
    }
}

