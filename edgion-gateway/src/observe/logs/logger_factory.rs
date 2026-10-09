//! Unified logger factory for creating different types of loggers

use std::sync::Arc;

use anyhow::Result;
use edgion_resources::{LocalFileWriterCfg, LocalFileWriterConfig, LogConfig, StringOutput};

use crate::link_sys::providers::elasticsearch::EsLogSender;
use crate::link_sys::providers::kafka::KafkaLogSender;
use crate::link_sys::providers::otlp::OtlpLogSender;
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

    writer_config = writer_config.with_disk_threshold(file_cfg.disk_usage_threshold_percent);

    writer_config
}

/// Build one initialized sender from a `StringOutput`. Mirrors the per-variant
/// init differences: LocalFile needs `init()` here; LinkSys outputs are lifecycle-
/// managed elsewhere and are not init'd here. Returns `None` for a disabled
/// LocalFile (empty path), matching the primary-path behavior.
async fn build_sender(output: &StringOutput, log_type: LogType) -> Result<Option<Box<dyn DataSender<String>>>> {
    match output {
        StringOutput::LocalFile(file_cfg) => {
            if file_cfg.path.is_empty() {
                tracing::info!(?log_type, "Sender disabled (empty path)");
                return Ok(None);
            }
            let writer_config = build_writer_config(file_cfg, log_type);
            let mut writer = LocalFileWriter::new(writer_config).with_log_type(log_type);
            writer.init().await?;
            Ok(Some(Box::new(writer)))
        }
        StringOutput::Es { link_sys_ref } => {
            let sender = EsLogSender::new(link_sys_ref.clone())?;
            tracing::info!(?log_type, link_sys_ref = sender.name(), "Sender: ES output");
            Ok(Some(Box::new(sender)))
        }
        StringOutput::Kafka { link_sys_ref, topic } => {
            let sender = KafkaLogSender::new(link_sys_ref.clone(), topic.clone())?;
            tracing::info!(?log_type, link_sys_ref = sender.name(), %topic, "Sender: Kafka output");
            Ok(Some(Box::new(sender)))
        }
        StringOutput::Otlp { link_sys_ref } => {
            let sender = OtlpLogSender::new(link_sys_ref.clone())?;
            tracing::info!(?log_type, link = sender.name(), "Sender: OTLP output");
            Ok(Some(Box::new(sender)))
        }
    }
}

/// Create an async logger (for Access, TCP, UDP, TLS logs).
/// Returns `None` if the log is disabled or the output is not supported.
pub async fn create_async_logger(config: &LogConfig, log_type: LogType) -> Result<Option<Arc<AccessLogger>>> {
    if !config.enabled {
        tracing::info!(?log_type, "Logger disabled by configuration");
        return Ok(None);
    }

    let Some(primary) = build_sender(&config.output, log_type).await? else {
        // Primary disabled (empty LocalFile path) — no logger.
        return Ok(None);
    };

    let mut logger = AccessLogger::new_with_log_type(log_type);
    logger.register(primary);

    // Optional fallback sender (first-healthy standby). A disabled fallback
    // (empty LocalFile path) is simply skipped — the primary still works.
    // An init failure from the fallback is not fatal: log a WARN and continue
    // with the primary rather than abandoning it.
    if let Some(fallback_cfg) = &config.fallback {
        match build_sender(fallback_cfg, log_type).await {
            Ok(Some(fallback)) => logger.register(fallback),
            Ok(None) => {} // disabled fallback (empty LocalFile path)
            Err(e) => {
                tracing::warn!(?log_type, error = %e, "Fallback sender init failed; continuing with primary only");
            }
        }
    }

    Ok(Some(Arc::new(logger)))
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
                link_sys_ref = link_sys_ref
                    .key()
                    .ok()
                    .as_deref()
                    .unwrap_or("invalid LinkSys reference"),
                "Es output is not supported for SSL logs (sync path); SSL logging disabled"
            );
            Ok(None)
        }
        StringOutput::Kafka { link_sys_ref, topic } => {
            tracing::warn!(
                link_sys_ref = link_sys_ref.key().ok().as_deref().unwrap_or("invalid LinkSys reference"),
                %topic,
                "Kafka output is not supported for SSL logs (sync path); SSL logging disabled"
            );
            Ok(None)
        }
        StringOutput::Otlp { link_sys_ref } => {
            let sender = OtlpLogSender::new(link_sys_ref.clone())?;
            tracing::warn!(
                link = sender.name(),
                "OTLP output is not supported for SSL logs (sync path); SSL logging disabled"
            );
            Ok(None)
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/observe/logs/logger_factory_test.rs"]
mod tests;
