//! Unified logger factory for creating different types of loggers

use anyhow::Result;
use std::sync::Arc;

use crate::core::gateway::link_sys::providers::local_file::LogType;
use crate::core::gateway::link_sys::{DataSender, LocalFileWriter};
use crate::core::gateway::observe::logs::ssl_log::SslLogger;
use crate::core::gateway::observe::AccessLogger;
use crate::types::{LocalFileWriterConfig, LogConfig, StringOutput};

/// Create an async logger (for Access, TCP, UDP logs)
/// Returns None if the log is disabled
pub async fn create_async_logger(config: &LogConfig, log_type: &str) -> Result<Option<Arc<AccessLogger>>> {
    if !config.enabled {
        tracing::info!(log_type, "Logger disabled by configuration");
        return Ok(None);
    }

    let mut logger = AccessLogger::new();

    match &config.output {
        StringOutput::LocalFile(file_cfg) => {
            // Check if path is empty
            if file_cfg.path.is_empty() {
                tracing::info!(log_type, "Logger disabled (empty path)");
                return Ok(None);
            }

            tracing::info!(
                log_type,
                path = %file_cfg.path,
                queue_size = ?file_cfg.queue_size,
                "Initializing logger with LocalFile output"
            );

            // Create LocalFileWriterConfig from config
            let mut writer_config = LocalFileWriterConfig::new(&file_cfg.path);

            if let Some(queue_size) = file_cfg.queue_size {
                writer_config = writer_config.with_queue_size(queue_size);
            }

            if let Some(rotation) = &file_cfg.rotation {
                writer_config = writer_config.with_rotation(rotation.clone());
            }

            // Determine log type from string
            let log_type_enum = match log_type {
                "access" => LogType::Access,
                "tls" => LogType::Tls,
                "tcp" => LogType::Tcp,
                "udp" => LogType::Udp,
                _ => LogType::Access,
            };

            // Create and initialize LocalFileWriter
            let mut writer = LocalFileWriter::new(writer_config).with_log_type(log_type_enum);
            writer.init().await?;

            // Register the writer
            logger.register(Box::new(writer));

            tracing::info!(log_type, "Logger initialized successfully");
        } // Future: Add support for other output types
          // StringOutput::Es(es_cfg) => { ... }
          // StringOutput::Kafka(kafka_cfg) => { ... }
    }

    Ok(Some(Arc::new(logger)))
}

/// Create a sync logger (for SSL log which needs sync API for TLS callbacks)
/// Returns None if the log is disabled
pub async fn create_sync_logger(config: &LogConfig) -> Result<Option<SslLogger>> {
    if !config.enabled {
        tracing::info!("SSL logger disabled by configuration");
        return Ok(None);
    }

    match &config.output {
        StringOutput::LocalFile(file_cfg) => {
            // Check if path is empty
            if file_cfg.path.is_empty() {
                tracing::info!("SSL logger disabled (empty path)");
                return Ok(None);
            }

            tracing::info!(
                path = %file_cfg.path,
                queue_size = ?file_cfg.queue_size,
                "Initializing SSL logger with LocalFile output"
            );

            // Create LocalFileWriterConfig from config
            let mut writer_config = LocalFileWriterConfig::new(&file_cfg.path);

            if let Some(queue_size) = file_cfg.queue_size {
                writer_config = writer_config.with_queue_size(queue_size);
            }

            if let Some(rotation) = &file_cfg.rotation {
                writer_config = writer_config.with_rotation(rotation.clone());
            }

            // Create LocalFileWriter (SslLogger will initialize it internally)
            let writer = LocalFileWriter::new(writer_config).with_log_type(LogType::Ssl);

            Ok(Some(SslLogger::new(writer)))
        } // Future: Add support for other output types
    }
}
