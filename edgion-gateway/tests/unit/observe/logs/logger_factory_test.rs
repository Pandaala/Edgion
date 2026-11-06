// `use super::*` brings in the gateway `LogType` that `create_async_logger` takes.
use super::*;
use edgion_resources::LinkSysRef;

#[cfg(feature = "otlp")]
fn assert_counter(rendered: &str, name: &str, value: u64) {
    let expected = format!("{name} {value}");
    assert!(
        rendered.lines().any(|line| line == expected),
        "expected {expected:?} in metrics:\n{rendered}"
    );
}

#[test]
#[cfg(feature = "otlp")]
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
#[cfg(feature = "otlp")]
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

#[tokio::test]
async fn programmatic_invalid_output_preserves_factory_failure_policy() {
    let reference = LinkSysRef {
        namespace: "bad/ns".into(),
        name: "sink".into(),
    };
    let outputs = [
        StringOutput::Es {
            link_sys_ref: reference.clone(),
        },
        StringOutput::Kafka {
            link_sys_ref: reference.clone(),
            topic: "topic".into(),
        },
        StringOutput::Otlp {
            link_sys_ref: reference,
        },
    ];
    for (index, output) in outputs.into_iter().enumerate() {
        let mut config = LogConfig {
            enabled: true,
            output: output.clone(),
            fallback: None,
        };
        let error = create_async_logger(&config, LogType::Access)
            .await
            .err()
            .expect("invalid primary");
        assert_eq!(error.to_string(), "invalid LinkSys reference");
        let sync = create_sync_logger(&config).await;
        if index == 2 {
            assert_eq!(
                sync.err()
                    .expect("OTLP validates before unsupported policy")
                    .to_string(),
                "invalid LinkSys reference"
            );
        } else {
            assert!(sync.unwrap().is_none());
        }
        config.enabled = false;
        assert!(create_async_logger(&config, LogType::Access).await.unwrap().is_none());
        assert!(create_sync_logger(&config).await.unwrap().is_none());
        config.enabled = true;
        config.fallback = Some(output);
        config.output = StringOutput::LocalFile(LocalFileWriterCfg {
            path: String::new(),
            ..Default::default()
        });
        assert!(create_async_logger(&config, LogType::Access).await.unwrap().is_none());
        let temp = tempfile::tempdir().unwrap();
        config.output = StringOutput::LocalFile(LocalFileWriterCfg {
            path: temp.path().join("primary.log").to_string_lossy().into_owned(),
            ..Default::default()
        });
        let logger = create_async_logger(&config, LogType::Access).await.unwrap().unwrap();
        assert_eq!(logger.sender_count(), 1, "invalid fallback must retain valid primary");
    }
}
