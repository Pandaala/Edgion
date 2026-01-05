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
pub async fn init_access_logger(config: &crate::core::cli::edgion_gateway::config::AccessLogConfig) -> Result<()> {
    let logger = create_access_logger(config).await?;
    ACCESS_LOGGER
        .set(logger)
        .map_err(|_| anyhow!("AccessLogger already initialized"))?;
    tracing::info!("Global AccessLogger initialized");
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

/// Create and initialize AccessLogger from configuration
/// Supports multiple output targets based on StringOutput enum
pub async fn create_access_logger(
    config: &crate::core::cli::edgion_gateway::config::AccessLogConfig,
) -> Result<Arc<AccessLogger>> {
    use crate::core::link_sys::DataSender;
    use crate::core::link_sys::LocalFileWriter;
    use crate::types::link_sys::{LocalFileWriterConfig, StringOutput};

    let mut logger = AccessLogger::new();

    // Process output configuration based on variant
    match &config.output {
        StringOutput::LocalFile(file_cfg) => {
            // Check for environment variable override first
            let log_path = std::env::var("EDGION_ACCESS_LOG").unwrap_or_else(|_| file_cfg.path.clone());

            // If path is empty, return empty logger
            if log_path.is_empty() {
                tracing::info!("Access logger disabled (no path configured)");
                return Ok(Arc::new(logger));
            }

            tracing::info!(
                path = %log_path,
                queue_size = ?file_cfg.queue_size,
                env_override = std::env::var("EDGION_ACCESS_LOG").is_ok(),
                "Initializing access logger with LocalFile output"
            );

            // Create LocalFileWriterConfig from config (with env override if present)
            let mut writer_config = LocalFileWriterConfig::new(&log_path);

            if let Some(queue_size) = file_cfg.queue_size {
                writer_config = writer_config.with_queue_size(queue_size);
            }

            if let Some(rotation) = &file_cfg.rotation {
                writer_config = writer_config.with_rotation(rotation.clone());
            }

            // Create and initialize LocalFileWriter
            let mut writer = LocalFileWriter::new(writer_config);
            writer.init().await?;

            // Register the writer
            logger.register(Box::new(writer));

            tracing::info!("Access logger initialized successfully with LocalFile output");
        } // Future: Add support for other output types
          // StringOutput::Es(es_cfg) => { ... }
          // StringOutput::Kafka(kafka_cfg) => { ... }
    }

    Ok(Arc::new(logger))
}
