//! Access log entry definition

use crate::types::{EdgionHttpContext, EdgionStatus, RequestInfo, UpstreamInfo};
use serde::Serialize;

/// Access log entry with only essential fields
#[derive(Serialize)]
pub struct AccessLogEntry<'a> {
    #[serde(rename = "ts")]
    pub timestamp: i64,
    
    #[serde(flatten)]
    pub request_info: &'a RequestInfo,
    
    pub errors: &'a [EdgionStatus],
    pub upstream: String,
    pub latency_ms: u64,
    
    #[serde(skip)]
    pub upstream_info: Option<&'a UpstreamInfo>,
}

impl<'a> AccessLogEntry<'a> {
    pub fn from_context(ctx: &'a EdgionHttpContext, latency_ms: u64) -> Self {
        let upstream = ctx.upstream_info.as_ref()
            .map(|u| format!("{}/{}/{}", u.namespace, u.name, u.peer))
            .unwrap_or_else(|| "-/-/-".to_string());
        
        Self {
            timestamp: chrono::Utc::now().timestamp_millis(),
            request_info: &ctx.request_info,
            errors: &ctx.error_codes,
            upstream,
            latency_ms,
            upstream_info: ctx.upstream_info.as_ref(),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|e| {
            tracing::error!("Failed to serialize access log: {}", e);
            "{}".to_string()
        })
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
            self.request_info.x_trace_id.as_deref().unwrap_or("-"),
        )
    }
}

