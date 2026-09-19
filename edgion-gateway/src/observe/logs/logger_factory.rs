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
            tracing::info!(?log_type, %link_sys_ref, "Sender: ES output");
            Ok(Some(Box::new(EsLogSender::new(link_sys_ref.clone()))))
        }
        StringOutput::Kafka { link_sys_ref, topic } => {
            tracing::info!(?log_type, %link_sys_ref, %topic, "Sender: Kafka output");
            Ok(Some(Box::new(KafkaLogSender::new(link_sys_ref.clone(), topic.clone()))))
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
mod tests {
    // `use super::*` brings in the gateway `LogType` that `create_async_logger` takes.
    use super::*;
    use edgion_resources::LinkSysRef;

    fn assert_counter(rendered: &str, name: &str, value: u64) {
        let expected = format!("{name} {value}");
        assert!(
            rendered.lines().any(|line| line == expected),
            "expected {expected:?} in metrics:\n{rendered}"
        );
    }

    #[test]
    fn async_logger_propagates_log_type_to_drop_metric() {
        const CHILD_ENV: &str = "EDGION_LOGGER_FACTORY_TYPED_DROP_TEST_CHILD";
        const TEST_MODULE: &str = "observe::logs::logger_factory::tests";
        const TEST_NAME: &str = "async_logger_propagates_log_type_to_drop_metric";

        crate::test_subprocess::assert_in_subprocess(TEST_MODULE, TEST_NAME, CHILD_ENV, TEST_NAME, || {
            let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
            let handle = recorder.handle();
            metrics::with_local_recorder(&recorder, || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("test runtime must build")
                    .block_on(async {
                        let config = LogConfig {
                            enabled: true,
                            output: StringOutput::Otlp {
                                link_sys_ref: LinkSysRef {
                                    namespace: "factory-typed-drop".into(),
                                    name: "missing-collector".into(),
                                },
                            },
                            fallback: None,
                        };
                        let logger = create_async_logger(&config, LogType::Udp)
                            .await
                            .expect("logger construction must succeed")
                            .expect("enabled output must produce a logger");
                        logger.send("dropped".into()).await;

                        let rendered = handle.render();
                        assert_counter(&rendered, "edgion_udp_log_dropped_total", 1);
                        assert!(!rendered.contains("edgion_access_log_dropped_total"));
                        assert!(!rendered.contains("edgion_tcp_log_dropped_total"));
                        assert!(!rendered.contains("edgion_tls_log_dropped_total"));
                    });
            });
        });
    }

    #[tokio::test]
    async fn missing_otlp_primary_starts_with_first_healthy_fallback() {
        let directory = tempfile::tempdir().unwrap();
        let config = LogConfig {
            enabled: true,
            output: StringOutput::Otlp {
                link_sys_ref: LinkSysRef {
                    namespace: "factory-test".into(),
                    name: "missing-collector".into(),
                },
            },
            fallback: Some(StringOutput::LocalFile(LocalFileWriterCfg {
                path: directory.path().join("fallback.log").to_string_lossy().into_owned(),
                disk_usage_threshold_percent: Some(0),
                ..Default::default()
            })),
        };
        let primary = build_sender(&config.output, LogType::Access).await.unwrap().unwrap();
        assert_eq!(primary.name(), "factory-test/missing-collector");
        assert!(!primary.healthy());
        let logger = create_async_logger(&config, LogType::Access).await.unwrap().unwrap();
        assert_eq!(logger.sender_count(), 2);
    }

    #[tokio::test]
    async fn invalid_programmatic_otlp_reference_never_becomes_a_sender() {
        let config = StringOutput::Otlp {
            link_sys_ref: LinkSysRef {
                namespace: "default".into(),
                name: "private-sentinel/invalid".into(),
            },
        };
        let error = build_sender(&config, LogType::Access).await.err().unwrap().to_string();
        assert!(!error.contains("private-sentinel"));
    }

    // A primary ES + LocalFile fallback must register TWO senders.
    #[tokio::test]
    async fn async_logger_registers_primary_then_fallback() {
        let temp_dir = tempfile::tempdir().unwrap();
        let log_path = temp_dir.path().join("fallback.log");
        let cfg = LogConfig {
            enabled: true,
            output: StringOutput::Es {
                link_sys_ref: LinkSysRef {
                    namespace: "default".into(),
                    name: "es-main".into(),
                },
            },
            fallback: Some(StringOutput::LocalFile(LocalFileWriterCfg {
                path: log_path.to_string_lossy().into_owned(),
                ..Default::default()
            })),
        };
        let logger = create_async_logger(&cfg, LogType::Access).await.unwrap().unwrap();
        assert_eq!(logger.sender_count(), 2, "primary + fallback both registered");
    }

    #[tokio::test]
    async fn async_logger_skips_disabled_fallback() {
        let temp_dir = tempfile::tempdir().unwrap();
        let log_path = temp_dir.path().join("primary.log");
        let cfg = LogConfig {
            enabled: true,
            output: StringOutput::LocalFile(LocalFileWriterCfg {
                path: log_path.to_string_lossy().into_owned(),
                ..Default::default()
            }),
            // Empty path => disabled fallback => skipped, primary still registered.
            fallback: Some(StringOutput::LocalFile(LocalFileWriterCfg {
                path: String::new(),
                ..Default::default()
            })),
        };
        let logger = create_async_logger(&cfg, LogType::Access).await.unwrap().unwrap();
        assert_eq!(
            logger.sender_count(),
            1,
            "disabled fallback is skipped; primary remains"
        );
    }
}
