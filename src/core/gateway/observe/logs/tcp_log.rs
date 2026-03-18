//! TCP connection logging

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::sync::{Arc, OnceLock};

use crate::core::gateway::observe::logs::create_async_logger;
use crate::core::gateway::observe::AccessLogger;
use crate::core::gateway::routes::tcp::edgion_tcp::TcpContext;

static TCP_LOGGER: OnceLock<Arc<AccessLogger>> = OnceLock::new();

#[derive(Serialize)]
pub struct TcpLogEntry {
    pub ts: i64,
    pub listener_port: u16,
    pub client_addr: String,
    pub client_port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_addr: Option<String>,
    pub duration_ms: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub status: String,
    pub connection_established: bool,
}

impl TcpLogEntry {
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

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

pub async fn init_tcp_logger(config: &crate::types::LogConfig) -> Result<()> {
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

pub fn get_tcp_logger() -> Option<&'static Arc<AccessLogger>> {
    TCP_LOGGER.get()
}

pub async fn log_tcp(entry: &TcpLogEntry) {
    if let Some(logger) = TCP_LOGGER.get() {
        let _ = logger.send(entry.to_json()).await;
    }
}
