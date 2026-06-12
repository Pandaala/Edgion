//! Access log module

mod entry;
mod logger;

pub use entry::AccessLogEntry;
pub use logger::AccessLogger;

use anyhow::{anyhow, Result};
use std::sync::{Arc, OnceLock};

use crate::observe::logs::{create_async_logger, LogType};

// Global AccessLogger instance (can only be initialized once)
static ACCESS_LOGGER: OnceLock<Arc<AccessLogger>> = OnceLock::new();

/// Initialize global AccessLogger from configuration.
///
/// Always sets `ACCESS_LOGGER` exactly once, even when access logging is
/// disabled by configuration (`enabled = false` or empty output path). In
/// the disabled case a no-op `AccessLogger` with zero senders is installed,
/// so downstream callers (e.g. startup listener configuration) can rely on
/// `get_access_logger_unchecked()` being safe after this returns.
pub async fn init_access_logger(config: &edgion_resources::LogConfig) -> Result<()> {
    let logger = match create_async_logger(config, LogType::Access).await? {
        Some(logger) => {
            tracing::info!("Global AccessLogger initialized");
            logger
        }
        None => {
            tracing::info!("AccessLogger disabled, installing no-op logger");
            Arc::new(AccessLogger::new())
        }
    };
    ACCESS_LOGGER
        .set(logger)
        .map_err(|_| anyhow!("AccessLogger already initialized"))?;
    Ok(())
}

/// Get global AccessLogger instance.
/// Returns `None` only if called before `init_access_logger`.
pub fn get_access_logger() -> Option<&'static Arc<AccessLogger>> {
    ACCESS_LOGGER.get()
}

/// Get global AccessLogger instance.
///
/// Safe to call any time after `init_access_logger` has returned `Ok`, which
/// is guaranteed by the gateway startup sequence before any listener wiring
/// or request processing begins. When access logging is disabled by config,
/// the returned logger is a no-op (zero senders) and `send` silently drops
/// without touching the `access_log_dropped` metric.
///
/// Panics if `init_access_logger` has not been called yet (programmer error).
pub fn get_access_logger_unchecked() -> &'static Arc<AccessLogger> {
    ACCESS_LOGGER.get().expect("AccessLogger not initialized")
}
