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
        "redis",
        "redis::cluster_handling::async_connection",
        "redis::aio::multiplexed_connection",
    ] {
        for level in [Level::TRACE, Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR] {
            assert!(!system_log_event_allowed(target, &level), "{target} {level}");
        }
    }
    for target in [
        "h2_extra",
        "rustls_client",
        "tonic::transport_extra",
        "redis_protocol",
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

/// Drives the real `init_logging` subscriber so the `log` -> tracing bridge it
/// installs is the path under test, matching how redis-rs emits records.
#[tokio::test]
async fn redis_sdk_log_records_are_dropped_through_the_log_bridge() {
    const CHILD_ENV: &str = "EDGION_REDIS_LOG_BRIDGE_TEST_CHILD";
    if std::env::var_os(CHILD_ENV).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "observe::logs::tests::redis_sdk_log_records_are_dropped_through_the_log_bridge",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .env("RUST_LOG", "trace,redis=trace")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        return;
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let config = LogConfig {
        log_dir: temp_dir.path().to_path_buf(),
        file_prefix: "redis-bridge".to_string(),
        json_format: false,
        console: false,
        level: "trace".to_string(),
    };
    let guard = init_logging(config).await.unwrap();

    log::warn!(target: "redis::cluster_handling::async_connection", "node-address-sentinel 10.0.0.7:6379");
    log::info!(target: "redis::cluster_handling::async_connection", "reconnect-sentinel");
    log::error!(target: "redis::aio::multiplexed_connection", "raw-error-sentinel");
    log::warn!(target: "edgion_gateway::link_sys::providers::redis", "redis_state_changed bridge-control");

    drop(guard);
    let output = std::fs::read_to_string(temp_dir.path().join("redis-bridge.log")).unwrap();
    assert!(!output.contains("sentinel"), "{output}");
    assert!(!output.contains("10.0.0.7"), "{output}");
    assert!(output.contains("bridge-control"), "{output}");
}

#[tokio::test]
async fn test_logging_from_different_threads() {
    const CHILD_ENV: &str = "EDGION_MULTITHREAD_LOGGING_TEST_CHILD";
    if std::env::var_os(CHILD_ENV).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "observe::logs::tests::test_logging_from_different_threads",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .env("RUST_LOG", "info")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        return;
    }

    // Installing the global subscriber is isolated to this child process.
    let temp_dir = tempfile::tempdir().unwrap();
    let config = LogConfig {
        log_dir: temp_dir.path().to_path_buf(),
        file_prefix: "multithread".to_string(),
        json_format: false,
        console: false,
        level: "info".to_string(),
    };

    let guard = init_logging(config).await.unwrap();

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

    // Both producers finished; dropping the worker guard flushes queued lines.
    drop(guard);
    let output = std::fs::read_to_string(temp_dir.path().join("multithread.log")).unwrap();
    assert!(output.contains("Log from tokio task"), "{output}");
    assert!(output.contains("Log from regular thread"), "{output}");
}
