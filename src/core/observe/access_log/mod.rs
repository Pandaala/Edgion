//! Access log module

mod entry;
mod logger;

pub use entry::AccessLogEntry;
pub use logger::AccessLogger;

use anyhow::{anyhow, Result};
use std::sync::{Arc, OnceLock};

// Global AccessLogger instance (can only be initialized once)
static ACCESS_LOGGER: OnceLock<Arc<AccessLogger>> = OnceLock::new();

/// Initialize global AccessLogger from configuration
/// Should be called once during application startup
/// Returns error if already initialized
pub async fn init_access_logger(config: &crate::types::LogConfig) -> Result<()> {
    use crate::core::observe::create_async_logger;

    if let Some(logger) = create_async_logger(config, "access").await? {
        ACCESS_LOGGER
            .set(logger)
            .map_err(|_| anyhow!("AccessLogger already initialized"))?;
        tracing::info!("Global AccessLogger initialized");
    } else {
        tracing::info!("AccessLogger disabled");
    }
    Ok(())
}

/// Get global AccessLogger instance
/// Returns None if not initialized
pub fn get_access_logger() -> Option<&'static Arc<AccessLogger>> {
    ACCESS_LOGGER.get()
}

/// Get global AccessLogger instance (panics if not initialized)
/// Use this when you're sure the logger has been initialized
pub fn get_access_logger_unchecked() -> &'static Arc<AccessLogger> {
    ACCESS_LOGGER.get().expect("AccessLogger not initialized")
}
