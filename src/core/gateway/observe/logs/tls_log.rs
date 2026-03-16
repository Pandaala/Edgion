//! TLS connection logging
//!
//! Provides an async logger for TLS proxy events. Callers pass their own
//! serializable context instead of converting into a dedicated log entry type.

use anyhow::{anyhow, Result};
use serde::Serialize;
use std::sync::{Arc, OnceLock};

use crate::core::gateway::observe::logs::create_async_logger;
use crate::core::gateway::observe::AccessLogger;

/// Global TLS logger instance
static TLS_LOGGER: OnceLock<Arc<AccessLogger>> = OnceLock::new();

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
pub async fn log_tls<T: Serialize>(entry: &T) {
    if let Some(logger) = TLS_LOGGER.get() {
        let payload = serde_json::to_string(entry).unwrap_or_else(|_| "{}".to_string());
        let _ = logger.send(payload).await;
    }
}
