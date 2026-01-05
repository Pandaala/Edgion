//! TCP connection logging

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::sync::{Arc, OnceLock};

use crate::core::observe::AccessLogger;
use crate::core::routes::tcp_routes::TcpContext;

/// Global TCP logger instance
static TCP_LOGGER: OnceLock<Arc<AccessLogger>> = OnceLock::new();

/// TCP connection log entry
#[derive(Serialize)]
pub struct TcpLogEntry {
    /// Timestamp in milliseconds
    pub ts: i64,
    /// Listener port
    pub listener_port: u16,
    /// Client address
    pub client_addr: String,
    /// Client port
    pub client_port: u16,
    /// Upstream address (if connection established)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_addr: Option<String>,
    /// Connection duration in milliseconds
    pub duration_ms: u64,
    /// Bytes sent to upstream
    pub bytes_sent: u64,
    /// Bytes received from upstream
    pub bytes_received: u64,
    /// Connection status
    pub status: String,
    /// Whether connection was successfully established
    pub connection_established: bool,
}

impl TcpLogEntry {
    /// Create a new TCP log entry from context
    pub fn from_context(ctx: &TcpContext) -> Self {
        let duration_ms = ctx.start_time.elapsed().as_millis() as u64;
        let status = format!("{:?}", ctx.status);

        Self {
            ts: chrono::Utc::now().timestamp_millis(),
            listener_port: ctx.listener_port,
            client_addr: ctx.client_addr.clone(),
            client_port: ctx.client_port,
            upstream_addr: ctx.upstream_addr.clone(),
            duration_ms,
            bytes_sent: ctx.bytes_sent,
            bytes_received: ctx.bytes_received,
            status,
            connection_established: ctx.connection_established,
        }
    }

    /// Serialize to JSON
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Initialize the global TCP logger from configuration
pub async fn init_tcp_logger(config: &crate::types::LogConfig) -> Result<()> {
    use crate::core::observe::create_async_logger;

    if let Some(logger) = create_async_logger(config, "tcp").await? {
        TCP_LOGGER
            .set(logger)
            .map_err(|_| anyhow!("TcpLogger already initialized"))?;
        tracing::info!("TcpLogger initialized");
    } else {
        tracing::info!("TcpLogger disabled");
    }
    Ok(())
}

/// Get global TCP logger instance
pub fn get_tcp_logger() -> Option<&'static Arc<AccessLogger>> {
    TCP_LOGGER.get()
}

/// Log a TCP connection entry (async)
pub async fn log_tcp(entry: &TcpLogEntry) {
    if let Some(logger) = TCP_LOGGER.get() {
        let _ = logger.send(entry.to_json()).await;
    }
}
