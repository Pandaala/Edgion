//! UDP session logging

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use crate::core::gateway::observe::logs::create_async_logger;
use crate::core::gateway::observe::AccessLogger;

/// Global UDP logger instance
static UDP_LOGGER: OnceLock<Arc<AccessLogger>> = OnceLock::new();

/// UDP session log entry
#[derive(Serialize)]
pub struct UdpLogEntry {
    /// Timestamp in milliseconds
    pub ts: i64,
    /// Listener port
    pub listener_port: u16,
    /// Client address
    pub client_addr: String,
    /// Client port
    pub client_port: u16,
    /// Upstream address (if known)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_addr: Option<String>,
    /// Session duration in milliseconds
    pub session_duration_ms: u64,
    /// Number of packets sent to upstream
    pub packets_sent: u64,
    /// Number of packets received from upstream
    pub packets_received: u64,
    /// Bytes sent to upstream
    pub bytes_sent: u64,
    /// Bytes received from upstream
    pub bytes_received: u64,
}

impl UdpLogEntry {
    /// Create a new UDP log entry
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
        }
    }

    /// Set packet/byte statistics
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

    /// Serialize to JSON
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
