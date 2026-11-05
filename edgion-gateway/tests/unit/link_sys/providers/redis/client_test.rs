use std::time::Duration;

use crate::link_sys::credential::CredentialState;
use edgion_resources::resources::link_sys::redis::RedisClientConfig;
use edgion_resources::resources::link_sys::redis::RedisPool;

use super::*;

#[test]
fn tls_verification_failure_stays_a_fixed_class() {
    let certificate = redis::RedisError::from(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "invalid peer certificate: UnknownIssuer",
    ));
    assert_eq!(
        initialization_failure_message(&certificate),
        "Redis TLS verification failed"
    );
    let refused = redis::RedisError::from(std::io::Error::new(
        std::io::ErrorKind::ConnectionRefused,
        "connection refused",
    ));
    assert_eq!(initialization_failure_message(&refused), "Redis initialization failed");
}

fn unavailable_error() -> redis::RedisError {
    redis::RedisError::from(std::io::Error::new(
        std::io::ErrorKind::NotConnected,
        "Redis unavailable",
    ))
}

#[test]
fn dial_split_prefers_tls_failure_for_classification() {
    let certificate = || {
        redis::RedisError::from(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid peer certificate: UnknownIssuer",
        ))
    };
    let refused = || {
        redis::RedisError::from(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "connection refused",
        ))
    };
    // Either completion order yields the TLS class.
    for results in [
        vec![Err(certificate()), Err(refused())],
        vec![Err(refused()), Err(certificate())],
    ] {
        let (ok, error) = super::super::backend::split_dial_results::<u8>(results);
        assert!(ok.is_empty());
        assert_eq!(
            initialization_failure_message(&error.unwrap()),
            "Redis TLS verification failed"
        );
    }
    let (_, error) = super::super::backend::split_dial_results::<u8>(vec![Err(refused()), Err(unavailable_error())]);
    assert!(
        error.unwrap().to_string().contains("connection refused"),
        "otherwise the first failure is kept"
    );
    let (ok, error) = super::super::backend::split_dial_results(vec![Ok(1_u8), Err(certificate())]);
    assert_eq!(ok, vec![1]);
    assert!(error.is_some());
}

/// Full establish -> classify path: every pool dial is rejected by an
/// untrusted certificate and init must report the TLS class, not the generic one.
/// Runs in its own process: client construction captures the process-global
/// outbound TLS default, which other lib tests publish and reset.
#[tokio::test]
async fn standalone_untrusted_certificate_reports_tls_verification_failure() {
    const CHILD_ENV: &str = "EDGION_REDIS_UNTRUSTED_TLS_CHILD";
    if std::env::var_os(CHILD_ENV).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "link_sys::providers::redis::client::tests::standalone_untrusted_certificate_reports_tls_verification_failure",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "isolated Redis TLS init test failed:\nstdout: {stdout}\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let _ = rustls::crypto::ring::default_provider().install_default();
    let certified = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()]).unwrap();
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der());
    let tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![certified.cert.der().clone()], key.into())
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(tls_config));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let _ = acceptor.accept(stream).await;
            });
        }
    });

    let config = RedisClientConfig {
        endpoints: vec![format!("rediss://127.0.0.1:{port}")],
        auth: None,
        db: None,
        timeout: None,
        pool: Some(RedisPool { size: Some(2) }),
        topology: None,
        tls: None,
    };
    let client =
        RedisLinkClient::from_test_config("test/untrusted-redis", &config, &CredentialState::NotRequired).unwrap();
    let error = tokio::time::timeout(Duration::from_secs(5), client.init())
        .await
        .expect("TLS rejection must finish before the test budget")
        .expect_err("an untrusted certificate must fail initialization");
    server.abort();

    assert_eq!(error.to_string(), "Redis TLS verification failed");
}

#[test]
fn only_a_replaced_primary_counts_as_a_failover() {
    let a = ("Primary-A".to_string(), 6379);
    let a_spelled = ("primary-a".to_string(), 6379);
    let b = ("primary-b".to_string(), 6379);
    assert!(primary_changed(Some(&a), Some(&b)));
    assert!(!primary_changed(Some(&a), Some(&a_spelled)));
    assert!(!primary_changed(None, Some(&a)));
    assert!(!primary_changed(Some(&a), None));
}

#[test]
fn partial_recovery_backs_off_and_full_recovery_resets() {
    let base = Duration::from_millis(100);
    assert_eq!(next_recovery_delay(base, true), base);
    assert_eq!(next_recovery_delay(base, false), Duration::from_millis(200));
    assert_eq!(
        next_recovery_delay(Duration::from_secs(20), false),
        Duration::from_secs(30)
    );
    assert_eq!(
        next_recovery_delay(Duration::from_secs(30), false),
        Duration::from_secs(30)
    );
}

#[test]
fn uninitialized_pool_has_no_connected_member() {
    let config = RedisClientConfig {
        endpoints: vec!["redis://127.0.0.1:6379".to_string()],
        auth: None,
        db: None,
        timeout: None,
        pool: Some(RedisPool { size: Some(2) }),
        topology: None,
        tls: None,
    };
    let client = RedisLinkClient::from_test_config("test/redis", &config, &CredentialState::NotRequired).unwrap();

    assert!(client.backend.snapshot.load().is_none());
    let coverage = client.connection_coverage();
    assert_eq!(coverage.configured, 2);
    assert_eq!(coverage.live, 0);
}

#[tokio::test]
async fn init_installs_egress_resolver_before_redis_connects() {
    let config = RedisClientConfig {
        endpoints: vec!["redis://169.254.169.254:6379".to_string()],
        auth: None,
        db: None,
        timeout: None,
        pool: Some(RedisPool { size: Some(1) }),
        topology: None,
        tls: None,
    };
    let client =
        RedisLinkClient::from_test_config("test/blocked-redis", &config, &CredentialState::NotRequired).unwrap();

    let error = tokio::time::timeout(Duration::from_secs(1), client.init())
        .await
        .expect("egress rejection must happen before the connection timeout")
        .expect_err("link-local Redis targets must be rejected");

    assert!(
        error.to_string().contains("Redis initialization failed"),
        "unexpected initialization error: {error}"
    );
}
