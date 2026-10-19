//! TCP connection logging

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::sync::{Arc, OnceLock};

use crate::observe::logs::{create_async_logger, LogType};
use crate::observe::AccessLogger;
use crate::routes::tcp::edgion_tcp::TcpContext;

static TCP_LOGGER: OnceLock<Arc<AccessLogger>> = OnceLock::new();

#[derive(Serialize)]
pub struct TcpLogEntry<'a> {
    pub ts: i64,
    pub listener_port: u16,
    #[serde(serialize_with = "edgion_resources::sensitive::masked::serialize")]
    pub direct_peer_ip: &'a str,
    pub direct_peer_port: u16,
    #[serde(serialize_with = "edgion_resources::sensitive::masked::serialize")]
    pub remote_ip: &'a str,
    pub remote_port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_addr: Option<&'a str>,
    pub duration_ms: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub status: &'static str,
    pub connection_established: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deny_reason: Option<&'static str>,
}

impl<'a> TcpLogEntry<'a> {
    pub fn from_context(ctx: &'a TcpContext) -> Self {
        let duration_ms = std::time::Instant::now()
            .saturating_duration_since(ctx.start_time)
            .as_millis() as u64;
        let status = ctx.status.as_str();

        Self {
            ts: chrono::Utc::now().timestamp_millis(),
            listener_port: ctx.listener_port,
            direct_peer_ip: &ctx.direct_peer_ip,
            direct_peer_port: ctx.direct_peer_port,
            remote_ip: &ctx.remote_ip,
            remote_port: ctx.remote_port,
            upstream_addr: ctx.upstream_addr.as_deref(),
            duration_ms,
            bytes_sent: ctx.bytes_sent,
            bytes_received: ctx.bytes_received,
            status,
            connection_established: ctx.connection_established,
            deny_reason: ctx.deny_reason,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

pub async fn init_tcp_logger(config: &edgion_resources::LogConfig) -> Result<()> {
    if let Some(logger) = create_async_logger(config, LogType::Tcp).await? {
        TCP_LOGGER
            .set(logger)
            .map_err(|_| anyhow!("TcpLogger already initialized"))?;
        tracing::info!("TcpLogger initialized");
    } else {
        tracing::info!("TcpLogger disabled");
    }
    Ok(())
}

pub async fn log_tcp(entry: &TcpLogEntry<'_>) {
    if let Some(logger) = TCP_LOGGER.get() {
        let _ = logger.send(entry.to_json()).await;
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/observe/logs/tcp_log_test.rs"]
mod tests;
