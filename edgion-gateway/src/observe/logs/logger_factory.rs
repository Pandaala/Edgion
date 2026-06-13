//! Unified logger factory for creating different types of loggers

use std::sync::Arc;

use anyhow::Result;
use edgion_resources::{LocalFileWriterCfg, LocalFileWriterConfig, LogConfig, StringOutput};

use crate::link_sys::providers::elasticsearch::EsLogSender;
use crate::link_sys::providers::kafka::KafkaLogSender;
use crate::link_sys::{DataSender, LocalFileWriter, LogType};
use crate::observe::logs::ssl_log::SslLogger;
use crate::observe::AccessLogger;

/// Build a `LocalFileWriterConfig` from the given `LocalFileWriterCfg`.
///
/// Caller is responsible for checking `enabled` and `path.is_empty()` before
/// calling this function.
fn build_writer_config(file_cfg: &LocalFileWriterCfg, log_type: LogType) -> LocalFileWriterConfig {
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

    writer_config
}

/// Create an async logger (for Access, TCP, UDP, TLS logs).
/// Returns `None` if the log is disabled or the output is not supported.
pub async fn create_async_logger(config: &LogConfig, log_type: LogType) -> Result<Option<Arc<AccessLogger>>> {
    if !config.enabled {
        tracing::info!(?log_type, "Logger disabled by configuration");
        return Ok(None);
    }

    match &config.output {
        StringOutput::LocalFile(file_cfg) => {
            if file_cfg.path.is_empty() {
                tracing::info!(?log_type, "Logger disabled (empty path)");
                return Ok(None);
            }
            let writer_config = build_writer_config(file_cfg, log_type);
            let mut writer = LocalFileWriter::new(writer_config).with_log_type(log_type);
            writer.init().await?;
            let mut logger = AccessLogger::new();
            logger.register(Box::new(writer));
            Ok(Some(Arc::new(logger)))
        }
        StringOutput::Es { link_sys_ref } => {
            tracing::info!(?log_type, %link_sys_ref, "Initializing logger with ES output");
            let sender = EsLogSender::new(link_sys_ref.clone());
            let mut logger = AccessLogger::new();
            logger.register(Box::new(sender));
            Ok(Some(Arc::new(logger)))
        }
        StringOutput::Kafka { link_sys_ref, topic } => {
            tracing::info!(?log_type, %link_sys_ref, %topic, "Initializing logger with Kafka output");
            let sender = KafkaLogSender::new(link_sys_ref.clone(), topic.clone());
            let mut logger = AccessLogger::new();
            logger.register(Box::new(sender));
            Ok(Some(Arc::new(logger)))
        }
    }
}

/// Create a sync logger (for SSL log which needs sync API for TLS callbacks).
/// Returns `None` if the log is disabled or the output is not supported.
pub async fn create_sync_logger(config: &LogConfig) -> Result<Option<SslLogger>> {
    if !config.enabled {
        tracing::info!("SSL logger disabled by configuration");
        return Ok(None);
    }
    match &config.output {
        StringOutput::LocalFile(file_cfg) => {
            if file_cfg.path.is_empty() {
                tracing::info!("SSL logger disabled (empty path)");
                return Ok(None);
            }
            let writer_config = build_writer_config(file_cfg, LogType::Ssl);
            let writer = LocalFileWriter::new(writer_config).with_log_type(LogType::Ssl);
            Ok(Some(SslLogger::new(writer)))
        }
        StringOutput::Es { link_sys_ref } => {
            tracing::warn!(
                %link_sys_ref,
                "Es output is not supported for SSL logs (sync path); SSL logging disabled"
            );
            Ok(None)
        }
        StringOutput::Kafka { link_sys_ref, topic } => {
            tracing::warn!(
                %link_sys_ref,
                %topic,
                "Kafka output is not supported for SSL logs (sync path); SSL logging disabled"
            );
            Ok(None)
        }
    }
}
