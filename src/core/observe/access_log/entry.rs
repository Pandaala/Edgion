//! Access log entry definition

use crate::types::{BackendContext, EdgionHttpContext, EdgionStatus, MatchInfo, RequestInfo};
use crate::core::plugins::StagePluginLogs;
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
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_info: Option<&'a MatchInfo>,
    
    #[serde(skip_serializing_if = "is_empty")]
    pub errors: &'a [EdgionStatus],
    
    pub backend_context: Option<&'a BackendContext>,
    
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub plugin_logs: &'a [StagePluginLogs],
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conn_est: Option<bool>,
}

impl<'a> AccessLogEntry<'a> {
    #[inline]
    pub fn from_context(ctx: &'a EdgionHttpContext) -> Self {
        let match_info = ctx.route_unit.as_ref().map(|ru| &ru.matched_info);
        
        Self {
            timestamp: chrono::Utc::now().timestamp_millis(),
            request_info: &ctx.request_info,
            match_info,
            errors: &ctx.error_codes,
            backend_context: ctx.backend_context.as_ref(),
            plugin_logs: &ctx.plugin_logs,
            conn_est: None,
        }
    }
    
    /// Set connection established flag
    #[inline]
    pub fn set_conn_est(&mut self) {
        self.conn_est = Some(true);
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|e| {
            tracing::error!("Failed to serialize access log: {}", e);
            "{}".to_string()
        })
    }

    pub fn to_combined(&self, latency_ms: u64) -> String {
        // Use the last upstream from backend_context for the combined log format
        let upstream_peer = self.backend_context
            .and_then(|bc| bc.upstreams.last())
            .map(|upstream| {
                let ip = upstream.ip.as_deref().unwrap_or("-");
                let port = upstream.port.map(|p| p.to_string()).unwrap_or_else(|| "-".to_string());
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
            self.request_info.status.map(|s| s.to_string()).unwrap_or_else(|| "-".to_string()),
            latency_ms,
            self.request_info.x_trace_id.as_deref().unwrap_or("-"),
        )
    }
}

