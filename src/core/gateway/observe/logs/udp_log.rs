//! UDP session logging

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use crate::core::gateway::observe::logs::create_async_logger;
use crate::core::gateway::observe::AccessLogger;

/// Global UDP logger instance
static UDP_LOGGER: OnceLock<Arc<AccessLogger>> = OnceLock::new();

/// UDP log entry (covers both session-based and per-packet failure logs)
#[derive(Serialize)]
pub struct UdpLogEntry {
    pub ts: i64,
    pub listener_port: u16,
    pub client_addr: String,
    pub client_port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_addr: Option<String>,
    pub session_duration_ms: u64,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

impl UdpLogEntry {
    pub fn new(
        listener_port: u16,
        client_addr: String,
        client_port: u16,
        upstream_addr: Option<String>,
        session_start: Instant,
    ) -> Self {
        let session_duration_ms = session_start.elapsed().as_millis() as u64;

        Self {
            ts: chrono::Utc::now().timestamp_millis(),
            listener_port,
            client_addr,
            client_port,
            upstream_addr,
            session_duration_ms,
            packets_sent: 0,
            packets_received: 0,
            bytes_sent: 0,
            bytes_received: 0,
            status: None,
        }
    }

    /// Create a failure log entry for packets that couldn't establish a session.
    pub fn failure(listener_port: u16, client_addr: String, client_port: u16, status: &str) -> Self {
        Self {
            ts: chrono::Utc::now().timestamp_millis(),
            listener_port,
            client_addr,
            client_port,
            upstream_addr: None,
            session_duration_ms: 0,
            packets_sent: 0,
            packets_received: 0,
            bytes_sent: 0,
            bytes_received: 0,
            status: Some(status.to_string()),
        }
    }

    pub fn with_stats(
        mut self,
        packets_sent: u64,
        packets_received: u64,
        bytes_sent: u64,
        bytes_received: u64,
    ) -> Self {
        self.packets_sent = packets_sent;
        self.packets_received = packets_received;
        self.bytes_sent = bytes_sent;
        self.bytes_received = bytes_received;
        self
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Initialize the global UDP logger from configuration
pub async fn init_udp_logger(config: &crate::types::LogConfig) -> Result<()> {
    if let Some(logger) = create_async_logger(config, "udp").await? {
        UDP_LOGGER
            .set(logger)
            .map_err(|_| anyhow!("UdpLogger already initialized"))?;
        tracing::info!("UdpLogger initialized");
    } else {
        tracing::info!("UdpLogger disabled");
    }
    Ok(())
}

/// Get global UDP logger instance
pub fn get_udp_logger() -> Option<&'static Arc<AccessLogger>> {
    UDP_LOGGER.get()
}

/// Log a UDP session entry (async)
pub async fn log_udp(entry: &UdpLogEntry) {
    if let Some(logger) = UDP_LOGGER.get() {
        let _ = logger.send(entry.to_json()).await;
    }
}
