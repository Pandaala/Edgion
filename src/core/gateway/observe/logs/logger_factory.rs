//! Unified logger factory for creating different types of loggers

use anyhow::Result;
use std::sync::Arc;

use crate::core::gateway::link_sys::providers::local_file::LogType;
use crate::core::gateway::link_sys::{DataSender, LocalFileWriter};
use crate::core::gateway::observe::logs::ssl_log::SslLogger;
use crate::core::gateway::observe::AccessLogger;
use edgion_resources::{LocalFileWriterConfig, LogConfig, StringOutput};

/// Build a `LocalFileWriterConfig` from the common `LogConfig` front-end.
///
/// Returns `None` when the logger is disabled by configuration or has an empty
/// output path (both are operator-driven "logger off" cases, not errors).
/// Future non-`LocalFile` `StringOutput` variants (Es, Kafka, ...) will need
/// their own front-end builder and should not be funnelled through this one.
fn build_writer_config(config: &LogConfig, log_type: LogType) -> Option<LocalFileWriterConfig> {
    if !config.enabled {
        tracing::info!(?log_type, "Logger disabled by configuration");
        return None;
    }

    let StringOutput::LocalFile(file_cfg) = &config.output;

    if file_cfg.path.is_empty() {
        tracing::info!(?log_type, "Logger disabled (empty path)");
        return None;
    }

    tracing::info!(
        ?log_type,
        path = %file_cfg.path,
        queue_size = ?file_cfg.queue_size,
        "Initializing logger with LocalFile output"
    );

    let mut writer_config = LocalFileWriterConfig::new(&file_cfg.path);

    if let Some(queue_size) = file_cfg.queue_size {
        writer_config = writer_config.with_queue_size(queue_size);
    }

    if let Some(rotation) = &file_cfg.rotation {
        writer_config = writer_config.with_rotation(rotation.clone());
    }

    Some(writer_config)
}

/// Create an async logger (for Access, TCP, UDP logs)
/// Returns None if the log is disabled
pub async fn create_async_logger(config: &LogConfig, log_type: LogType) -> Result<Option<Arc<AccessLogger>>> {
    let Some(writer_config) = build_writer_config(config, log_type) else {
        return Ok(None);
    };

    // Create and initialize LocalFileWriter
    let mut writer = LocalFileWriter::new(writer_config).with_log_type(log_type);
    writer.init().await?;

    // Register the writer
    let mut logger = AccessLogger::new();
    logger.register(Box::new(writer));

    tracing::info!(?log_type, "Logger initialized successfully");

    Ok(Some(Arc::new(logger)))
}

/// Create a sync logger (for SSL log which needs sync API for TLS callbacks)
/// Returns None if the log is disabled
pub async fn create_sync_logger(config: &LogConfig) -> Result<Option<SslLogger>> {
    let Some(writer_config) = build_writer_config(config, LogType::Ssl) else {
        return Ok(None);
    };

    // Create LocalFileWriter (SslLogger will initialize it internally)
    let writer = LocalFileWriter::new(writer_config).with_log_type(LogType::Ssl);

    Ok(Some(SslLogger::new(writer)))
}
