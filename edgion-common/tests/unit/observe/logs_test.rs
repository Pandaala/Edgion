use super::*;

#[test]
fn sensitive_transport_targets_are_closed_at_every_level() {
    for target in [
        "tonic::transport",
        "tonic::transport::channel::service::reconnect",
        "rustls",
        "rustls::client::tls12",
        "h2",
        "h2::codec::framed_read",
    ] {
        for level in [Level::TRACE, Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR] {
            assert!(!system_log_event_allowed(target, &level), "{target} {level}");
        }
    }
    for target in [
        "h2_extra",
        "rustls_client",
        "tonic::transport_extra",
        "edgion_gateway::link_sys",
    ] {
        assert!(system_log_event_allowed(target, &Level::TRACE), "{target}");
    }
}

#[test]
fn protected_transport_filter_cannot_be_overridden_by_trace_directives() {
    use parking_lot::Mutex;
    use std::io::Write;
    use std::sync::Arc;

    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let captured = Arc::new(Mutex::new(Vec::new()));
    let writer = Capture(captured.clone());
    let subscriber = tracing_subscriber::registry()
        .with(EnvFilter::new("trace,tonic::transport=trace,rustls=trace,h2=trace"))
        .with(filter_fn(|metadata| {
            system_log_event_allowed(metadata.target(), metadata.level())
        }))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(move || writer.clone()),
        );
    tracing::subscriber::with_default(subscriber, || {
        tracing::debug!(target: "tonic::transport::channel::service::reconnect", "tls-identity-sentinel");
        tracing::warn!(target: "rustls::client::hs", "certificate-sentinel");
        tracing::error!(target: "h2::codec", "collector-error-sentinel");
        tracing::info!(target: "edgion_gateway::link_sys", link = "default/collector", "connect_failed");
    });
    let output = String::from_utf8(captured.lock().clone()).unwrap();
    assert!(!output.contains("sentinel"));
    assert!(output.contains("default/collector"));
    assert!(output.contains("connect_failed"));
}

#[test]
fn pingora_h1_header_log_targets_have_an_error_floor() {
    for target in [
        PINGORA_H1_CLIENT_TARGET,
        "pingora_core::protocols::http::v1::client::nested",
        PINGORA_H1_PROXY_TARGET,
        "pingora_proxy::proxy_h1::nested",
    ] {
        assert!(!system_log_event_allowed(target, &Level::TRACE));
        assert!(!system_log_event_allowed(target, &Level::DEBUG));
        assert!(!system_log_event_allowed(target, &Level::INFO));
        assert!(!system_log_event_allowed(target, &Level::WARN));
        assert!(system_log_event_allowed(target, &Level::ERROR));
    }

    assert!(system_log_event_allowed(
        "pingora_core::connectors::http",
        &Level::TRACE
    ));
    assert!(system_log_event_allowed("edgion_common::observe", &Level::TRACE));
}

#[tokio::test]
async fn test_logging_from_different_threads() {
    let temp_dir = std::env::temp_dir().join("edgion_test_multithread");
    let config = LogConfig {
        log_dir: temp_dir,
        file_prefix: "multithread".to_string(),
        json_format: false,
        console: false,
        level: "info".to_string(),
    };

    let _guard = init_logging(config).await.unwrap();

    tokio::spawn(async {
        tracing::info!("Log from tokio task");
    })
    .await
    .unwrap();

    std::thread::spawn(|| {
        tracing::info!("Log from regular thread");
    })
    .join()
    .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
}
