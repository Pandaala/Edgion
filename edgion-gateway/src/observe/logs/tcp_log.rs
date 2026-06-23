//! TCP connection logging

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::sync::{Arc, OnceLock};

use crate::observe::logs::{create_async_logger, LogType};
use crate::observe::AccessLogger;
use crate::routes::tcp::edgion_tcp::TcpContext;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deny_reason: Option<&'static str>,
}

impl TcpLogEntry {
    pub fn from_context(ctx: &TcpContext) -> Self {
        let duration_ms = std::time::Instant::now()
            .saturating_duration_since(ctx.start_time)
            .as_millis() as u64;
        let status = ctx.status.as_str().to_owned();

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

pub fn get_tcp_logger() -> Option<&'static Arc<AccessLogger>> {
    TCP_LOGGER.get()
}

pub async fn log_tcp(entry: &TcpLogEntry) {
    if let Some(logger) = TCP_LOGGER.get() {
        let _ = logger.send(entry.to_json()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::tcp::edgion_tcp::TcpStatus;
    use std::time::Instant;

    fn denied_ctx(reason: Option<&'static str>) -> TcpContext {
        TcpContext {
            listener_port: 9000,
            client_addr: "127.0.0.1".to_string(),
            client_port: 50000,
            upstream_addr: None,
            start_time: Instant::now(),
            bytes_sent: 0,
            bytes_received: 0,
            status: TcpStatus::PluginDenied,
            connection_established: false,
            deny_reason: reason,
        }
    }

    #[test]
    fn from_context_propagates_deny_reason_into_json() {
        // A plugin-denied connection (connection_established = false) is still
        // logged, and its bounded deny_reason must reach the JSON entry.
        let entry = TcpLogEntry::from_context(&denied_ctx(Some("stream plugin resource not found")));
        assert_eq!(entry.deny_reason, Some("stream plugin resource not found"));
        assert!(
            entry
                .to_json()
                .contains(r#""deny_reason":"stream plugin resource not found""#),
            "deny_reason must be serialized into the TCP log JSON"
        );
    }

    #[test]
    fn from_context_omits_deny_reason_when_none() {
        // Non-denied connections leave deny_reason None; serde must skip it.
        let entry = TcpLogEntry::from_context(&denied_ctx(None));
        assert_eq!(entry.deny_reason, None);
        assert!(
            !entry.to_json().contains("deny_reason"),
            "deny_reason must be omitted from JSON when None"
        );
    }

    #[test]
    fn tcp_status_as_str_matches_log_field() {
        // Locks the TCP access log `status` field labels. If a variant is
        // renamed for code clarity, this test forces an explicit update so
        // operators' log pipelines do not break silently.
        let cases: &[(TcpStatus, &str)] = &[
            (TcpStatus::Success, "Success"),
            (TcpStatus::NoRouteMatched, "NoRouteMatched"),
            (TcpStatus::NoRuleAvailable, "NoRuleAvailable"),
            (TcpStatus::PluginDenied, "PluginDenied"),
            (TcpStatus::NoBackendSelected, "NoBackendSelected"),
            (TcpStatus::NoBackendResolved, "NoBackendResolved"),
            (TcpStatus::UnsafeUpstreamAddress, "UnsafeUpstreamAddress"),
            (TcpStatus::UpstreamConnectionFailed, "UpstreamConnectionFailed"),
            (TcpStatus::UpstreamReadError, "UpstreamReadError"),
            (TcpStatus::UpstreamWriteError, "UpstreamWriteError"),
            (TcpStatus::DownstreamReadError, "DownstreamReadError"),
            (TcpStatus::DownstreamWriteError, "DownstreamWriteError"),
            (TcpStatus::IdleTimeout, "IdleTimeout"),
        ];
        for (variant, expected) in cases {
            assert_eq!(variant.as_str(), *expected);
        }
    }
}
