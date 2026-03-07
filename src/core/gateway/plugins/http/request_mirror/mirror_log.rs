use chrono::Utc;
use serde::Serialize;

use crate::core::gateway::observe::access_log::get_access_logger;

#[derive(Serialize)]
pub struct MirrorLogEntry {
    #[serde(rename = "type")]
    pub entry_type: &'static str,
    #[serde(rename = "ts")]
    pub timestamp: i64,
    #[serde(rename = "x-trace-id")]
    pub x_trace_id: String,
    pub target: String,
    pub status: &'static str,
    pub et: u64,
    pub bytes_sent: u64,
    pub chunks_sent: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub async fn emit_mirror_log(entry: MirrorLogEntry) {
    if let Some(logger) = get_access_logger() {
        match serde_json::to_string(&entry) {
            Ok(data) => logger.send(data).await,
            Err(e) => tracing::warn!(error = %e, "failed to serialize mirror log"),
        }
    }
}

pub fn new_entry(
    x_trace_id: String,
    target: String,
    status: &'static str,
    et: u64,
    bytes_sent: u64,
    chunks_sent: u64,
    error: Option<String>,
) -> MirrorLogEntry {
    MirrorLogEntry {
        entry_type: "mirror",
        timestamp: Utc::now().timestamp_millis(),
        x_trace_id,
        target,
        status,
        et,
        bytes_sent,
        chunks_sent,
        error,
    }
}
