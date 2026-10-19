use chrono::Utc;
use serde::Serialize;

use crate::observe::access_log::get_access_logger;

#[derive(Serialize)]
pub struct MirrorLogEntry {
    #[serde(rename = "type")]
    pub entry_type: &'static str,
    #[serde(rename = "ts")]
    pub timestamp: i64,
    pub trace_id: String,
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
        if logger.sender_count() == 0 {
            return;
        }
        if let Ok(data) = serde_json::to_string(&entry) {
            logger.send(data).await
        }
    }
}

pub fn new_entry(
    trace_id: String,
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
        trace_id,
        target,
        status,
        et,
        bytes_sent,
        chunks_sent,
        error,
    }
}
