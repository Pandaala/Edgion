use std::sync::atomic::AtomicUsize;

use super::*;

impl EtcdLinkClient {
    pub(crate) fn samples_endpoint_coverage(&self) -> bool {
        self.sample_endpoint_coverage
    }
}

fn minimal_config() -> EtcdClientConfig {
    EtcdClientConfig {
        endpoints: vec!["http://localhost:2379".to_string()],
        auth: None,
        tls: None,
        timeout: None,
        keep_alive: None,
        namespace: None,
        auto_sync_interval: None,
        max_call_send_size: None,
        max_call_recv_size: None,
        user_agent: None,
        reject_old_cluster: None,
        observability: None,
    }
}

fn no_auth() -> CredentialState<BasicAuthMaterial> {
    CredentialState::NotRequired
}

#[test]
fn test_from_config_minimal() {
    let config = minimal_config();
    let client = EtcdLinkClient::from_config("default/etcd-test", &config, &no_auth());
    assert!(client.is_ok());
    let client = client.unwrap();
    assert_eq!(client.name(), "default/etcd-test");
    assert!(!client.healthy());
    assert!(client.namespace().is_none());
}

#[test]
fn test_from_config_with_namespace() {
    let config = EtcdClientConfig {
        namespace: Some("/app/".to_string()),
        ..minimal_config()
    };
    let client = EtcdLinkClient::from_config("default/etcd-ns", &config, &no_auth()).unwrap();
    assert_eq!(client.namespace(), Some("/app/"));
    assert_eq!(client.full_key("mykey"), "/app/mykey");
}

#[test]
fn test_from_config_without_namespace() {
    let config = minimal_config();
    let client = EtcdLinkClient::from_config("default/etcd-test", &config, &no_auth()).unwrap();
    assert_eq!(client.full_key("mykey"), "mykey");
}

#[test]
fn test_empty_endpoints_returns_error() {
    let config = EtcdClientConfig {
        endpoints: vec![],
        ..minimal_config()
    };
    let result = EtcdLinkClient::from_config("test", &config, &no_auth());
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("empty"));
}

/// Defence-in-depth: `from_config` must refuse `Pending` / `Invalid` even
/// though dispatch normally filters these out up-front.
#[test]
fn test_from_config_pending_state_fails_fast() {
    let config = minimal_config();
    let result = EtcdLinkClient::from_config("default/etcd-test", &config, &CredentialState::Pending);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("non-buildable"));
}

#[test]
fn controller_construction_does_not_enable_endpoint_coverage_sampling() {
    let client = EtcdLinkClient::from_config("default/etcd-test", &minimal_config(), &no_auth()).unwrap();
    assert!(!client.samples_endpoint_coverage());
    assert_eq!(client.endpoint_coverage(), (0, 1));
    let (rebuild, secret, rv) = client.apply_fingerprint_parts();
    assert_eq!(rebuild, [0; 32]);
    assert_eq!(secret, [0; 32]);
    assert!(rv.is_none());
}

#[test]
fn gateway_apply_meta_attaches_fingerprint_and_enables_sampling() {
    let rebuild = [7u8; 32];
    let secret = [9u8; 32];
    let client = EtcdLinkClient::from_config("default/etcd-gw", &minimal_config(), &no_auth())
        .unwrap()
        .with_gateway_apply_meta(rebuild, secret, Some("11".to_string()));
    assert!(client.samples_endpoint_coverage());
    assert_eq!(
        client.apply_fingerprint_parts(),
        (rebuild, secret, Some("11".to_string()))
    );
    assert_eq!(client.endpoint_coverage(), (0, 1));
}

#[test]
fn etcd_reports_only_published_health_transitions() {
    let recovered = Arc::new(AtomicUsize::new(0));
    let degraded = Arc::new(AtomicUsize::new(0));
    let recovered_reporter = Arc::clone(&recovered);
    let degraded_reporter = Arc::clone(&degraded);
    let reporter: HealthTransitionReporter = Arc::new(move |healthy| {
        if healthy {
            recovered_reporter.fetch_add(1, Ordering::Relaxed);
        } else {
            degraded_reporter.fetch_add(1, Ordering::Relaxed);
        }
    });
    let client = EtcdLinkClient::from_config("default/etcd-health", &minimal_config(), &no_auth())
        .unwrap()
        .with_health_transition_reporter(reporter);

    assert!(client.published_health().update(true, false));
    assert_eq!(recovered.load(Ordering::Relaxed), 0);
    PublishedHealth::transition_publication(None, Some(client.published_health()), || {});
    assert!(client.published_health().update(false, false));
    assert!(!client.published_health().update(false, false));
    assert!(client.published_health().update(true, false));
    assert_eq!(degraded.load(Ordering::Relaxed), 1);
    assert_eq!(recovered.load(Ordering::Relaxed), 1);

    client.mark_retired();
    assert!(client.published_health().update(false, false));
    assert_eq!(degraded.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn connection_options_errors_prevent_all_dials() {
    use edgion_resources::resources::common::{OutboundTlsLocal, TlsConfig};
    let _guard = crate::link_sys_core::outbound_tls::outbound_tls_test_guard();
    let mut config = minimal_config();
    config.tls = Some(TlsConfig {
        enabled: true,
        outbound: OutboundTlsLocal {
            verify: Some(false),
            ..Default::default()
        },
        resolved_ca_certificates: None,
        resolved_client_certificate: None,
    });
    // No endpoints can be dialed; the TLS error must precede endpoint validation.
    let Err(error) = connect_with_config(&[], &config, &no_auth()).await else {
        panic!("invalid TLS options must prevent connection");
    };
    assert!(error.to_string().contains("OutboundTlsVerifyUnsupported"));
}
