//! In-process certificate rotation and real metrics-router regression. No extra
//! Gateway/Controller topology or live peer is needed: Channels stay lazy.

use std::sync::Arc;
use std::time::Duration;

use axum::body::to_bytes;
use edgion_common::conf_sync::cert_expiry_watch::{CERT_EXPIRY_METRIC, ERROR_THRESHOLD};
use edgion_common::conf_sync::config::{ConfSyncSecurityConfig, ConfSyncTlsConfig};
use edgion_gateway::cli::config::MetricsConfig;
use edgion_gateway::conf_sync::ConfigSyncClient;
use edgion_gateway::observe::metrics::api::{create_metrics_router, init_metrics_exporter};
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use tower::ServiceExt;

async fn scrape(router: &axum::Router) -> String {
    let response = router
        .clone()
        .oneshot(
            http::Request::builder()
                .uri("/metrics")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::OK);
    String::from_utf8(to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap()
}

fn expiry(body: &str) -> Option<i64> {
    body.lines()
        .find(|line| line.starts_with(CERT_EXPIRY_METRIC) && line.contains("side=\"client\""))
        .map(|line| line.split_whitespace().last().unwrap().parse::<f64>().unwrap() as i64)
}

#[tokio::test]
async fn scrapes_follow_installed_identity_through_stalled_rejected_and_successful_reload() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    init_metrics_exporter(&MetricsConfig::default()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let tls = ConfSyncTlsConfig {
        name: None,
        cert: dir.path().join("client.crt").to_str().unwrap().into(),
        key: dir.path().join("client.key").to_str().unwrap().into(),
        ca: dir.path().join("ca.crt").to_str().unwrap().into(),
        skip_tls: false,
    };
    let mut security = ConfSyncSecurityConfig {
        certs: vec![tls.clone()],
        ..Default::default()
    };
    let addresses = vec!["localhost:65000".into()];

    // Missing auto-CA material cannot publish an invented installed expiry.
    security.auto_ca = true;
    assert!(
        ConfigSyncClient::new(addresses.clone(), "test".into(), Duration::from_secs(1), &security)
            .await
            .is_err()
    );
    security.auto_ca = false;
    let mut plaintext = security.clone();
    plaintext.certs[0].skip_tls = true;
    let plain = Arc::new(
        ConfigSyncClient::new(addresses.clone(), "test".into(), Duration::from_secs(1), &plaintext)
            .await
            .unwrap(),
    );
    assert_eq!(plain.installed_cert_validity(), None);
    assert_eq!(expiry(&scrape(&create_metrics_router(plain)).await), None);

    let ca_key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = params.self_signed(&ca_key).unwrap();
    let issuer = Issuer::from_ca_cert_pem(&ca.pem(), ca_key).unwrap();
    let key_a = KeyPair::generate().unwrap();
    let key_b = KeyPair::generate().unwrap();
    let now = time::OffsetDateTime::now_utc();
    let mut params = CertificateParams::new(vec!["localhost".into()]).unwrap();
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(3);
    let expires_a = params.not_after.unix_timestamp();
    let cert_a = params.signed_by(&key_a, &issuer).unwrap();
    params.not_after = now + time::Duration::days(365);
    let expires_b = params.not_after.unix_timestamp();
    let cert_b = params.signed_by(&key_b, &issuer).unwrap();
    // tonic ignores other PEM labels and uses the first CERTIFICATE as leaf.
    // A long-lived public-key-labeled certificate must not supply A's expiry.
    let disguised_b = cert_b.pem().replace("CERTIFICATE", "PUBLIC KEY");
    let mixed_a = format!("{disguised_b}{}{}{}", key_a.serialize_pem(), cert_a.pem(), ca.pem());
    std::fs::write(tls.cert_path(), mixed_a).unwrap();
    std::fs::write(tls.key_path(), key_a.serialize_pem()).unwrap();
    std::fs::write(tls.ca_path(), ca.pem()).unwrap();
    let client = Arc::new(
        ConfigSyncClient::new(addresses, "test".into(), Duration::from_secs(1), &security)
            .await
            .unwrap(),
    );
    let router = create_metrics_router(client.clone());
    assert_eq!(expiry(&scrape(&router).await), Some(expires_a));
    assert!(expires_a - now.unix_timestamp() < ERROR_THRESHOLD.as_secs() as i64);

    // A healthy disk replacement without a running watcher is still uninstalled.
    std::fs::write(tls.cert_path(), cert_b.pem()).unwrap();
    std::fs::write(tls.key_path(), key_b.serialize_pem()).unwrap();
    assert_eq!(expiry(&scrape(&router).await), Some(expires_a));

    // Parsing B succeeds, but mismatched credentials must retain both A's
    // Channel and A's alert value; a repeated attempt has the same outcome.
    std::fs::write(tls.key_path(), key_a.serialize_pem()).unwrap();
    for _ in 0..2 {
        assert!(client.reload_client_tls().await.is_err());
        assert_eq!(client.installed_cert_validity().unwrap().not_after, expires_a);
        assert_eq!(expiry(&scrape(&router).await), Some(expires_a));
    }
    std::fs::write(tls.key_path(), key_b.serialize_pem()).unwrap();
    client.reload_client_tls().await.unwrap();
    assert_eq!(client.installed_cert_validity().unwrap().not_after, expires_b);
    assert_eq!(expiry(&scrape(&router).await), Some(expires_b));

    // Scrapes read process state even when all backing files have disappeared.
    std::fs::remove_file(tls.cert_path()).unwrap();
    std::fs::remove_file(tls.key_path()).unwrap();
    std::fs::remove_file(tls.ca_path()).unwrap();
    assert!(client.reload_client_tls().await.is_err());
    assert_eq!(expiry(&scrape(&router).await), Some(expires_b));
}
