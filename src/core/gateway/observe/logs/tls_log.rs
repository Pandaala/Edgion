//! TLS connection logging
//!
//! Provides structured logging for TLS-terminated connections with two log events
//! per connection: "connect" (upstream established) and "disconnect" (session ended).

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::sync::{Arc, OnceLock};

use crate::core::gateway::observe::logs::create_async_logger;
use crate::core::gateway::observe::AccessLogger;

/// Global TLS logger instance
static TLS_LOGGER: OnceLock<Arc<AccessLogger>> = OnceLock::new();

/// TLS connection log entry
#[derive(Serialize)]
pub struct TlsLogEntry {
    pub ts: i64,
    pub event: String,
    pub protocol: String,
    pub listener_port: u16,
    pub client_addr: String,
    pub client_port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sni_hostname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_addr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_sent: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_received: Option<u64>,
    pub status: String,
    pub connection_established: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_name: Option<String>,
}

impl TlsLogEntry {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Initialize the global TLS logger from configuration
pub async fn init_tls_logger(config: &crate::types::LogConfig) -> Result<()> {
    if let Some(logger) = create_async_logger(config, "tls").await? {
        TLS_LOGGER
            .set(logger)
            .map_err(|_| anyhow!("TlsLogger already initialized"))?;
        tracing::info!("TlsLogger initialized");
    } else {
        tracing::info!("TlsLogger disabled");
    }
    Ok(())
}

/// Get global TLS logger instance
pub fn get_tls_logger() -> Option<&'static Arc<AccessLogger>> {
    TLS_LOGGER.get()
}

/// Log a TLS connection entry (async)
pub async fn log_tls(entry: &TlsLogEntry) {
    if let Some(logger) = TLS_LOGGER.get() {
        let _ = logger.send(entry.to_json()).await;
    }
}
