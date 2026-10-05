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
        min_cluster_version: None,
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
    assert_eq!(client.namespace(), Some("/app/"));
}

#[test]
fn test_from_config_without_namespace() {
    let config = minimal_config();
    let client = EtcdLinkClient::from_config("default/etcd-test", &config, &no_auth()).unwrap();
    assert!(client.namespace().is_none());
}

#[test]
fn test_empty_endpoints_returns_error() {
    let config = EtcdClientConfig {
        endpoints: vec![],
        ..minimal_config()
    };
    let result = EtcdLinkClient::from_config("test", &config, &no_auth());
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("no endpoints"));
}

#[test]
fn test_from_config_rejects_invalid_message_policy_before_init() {
    let mut config = minimal_config();
    config.max_call_recv_size = Some(0);
    let result = EtcdLinkClient::from_config("test", &config, &no_auth());
    assert!(result.unwrap_err().to_string().contains("maxCallRecvSize"));
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
    config.endpoints = vec!["https://localhost:2379".to_owned()];
    // TLS policy is validated synchronously before any connection can be created.
    let Err(error) = EtcdRuntimePolicy::resolve(&config, &no_auth()) else {
        panic!("invalid TLS options must prevent connection");
    };
    assert!(error.to_string().contains("OutboundTlsVerifyUnsupported"));
}

#[derive(Default)]
struct TestObservation {
    completions: parking_lot::Mutex<Vec<(Operation, super::super::observation::Outcome)>>,
    maintenance: parking_lot::Mutex<Vec<MaintenanceEvent>>,
    retired: AtomicBool,
}
impl EtcdObservation for TestObservation {
    fn completed(&self, operation: Operation, outcome: super::super::observation::Outcome) {
        self.completions.lock().push((operation, outcome));
    }
    fn maintenance(&self, event: MaintenanceEvent) {
        self.maintenance.lock().push(event);
    }
    fn deactivate(&self) {
        self.retired.store(true, Ordering::Relaxed);
    }
}

#[tokio::test]
async fn common_preflights_report_the_correct_service_before_value_conversion() {
    use super::super::observation::Outcome;
    struct BorrowedOnly;
    impl AsRef<[u8]> for BorrowedOnly {
        fn as_ref(&self) -> &[u8] {
            b"oversized"
        }
    }
    impl From<BorrowedOnly> for Vec<u8> {
        fn from(_: BorrowedOnly) -> Self {
            panic!("oversized borrowed values must never be converted")
        }
    }
    let mut config = minimal_config();
    config.max_call_send_size = Some(2);
    let observation = Arc::new(TestObservation::default());
    let client = EtcdLinkClient::from_config("test/observed", &config, &no_auth())
        .unwrap()
        .with_observation(observation.clone(), true);
    // An empty admitted set creates the SDK facade without a network peer.
    // Every operation below must be rejected locally, before dispatch/dial.
    let set = client.new_endpoint_set(false).unwrap();
    *client.client.write().await = Some(set.client().unwrap());
    assert!(client.put("k", BorrowedOnly, None).await.is_err());
    assert!(client.watch_key_from_revision("oversized", 0).await.is_err());
    assert!(client.try_lock("oversized", 1, Duration::from_secs(1)).await.is_err());
    let mut election = crate::link_sys_core::providers::etcd::ElectionSession::new(set.client().unwrap());
    assert!(election.leader("oversized").await.is_err());
    assert_eq!(
        *observation.completions.lock(),
        vec![
            (Operation::Kv, Outcome::SendLimit),
            (Operation::Watch, Outcome::SendLimit),
            (Operation::Lock, Outcome::SendLimit),
            (Operation::Election, Outcome::SendLimit),
        ]
    );
}

#[tokio::test]
async fn maintenance_only_observer_has_no_sdk_completion_bridge() {
    let mut config = minimal_config();
    config.max_call_send_size = Some(1);
    let observation = Arc::new(TestObservation::default());
    let client = EtcdLinkClient::from_config("test/maintenance", &config, &no_auth())
        .unwrap()
        .with_observation(observation.clone(), false);
    let sdk = client.new_endpoint_set(false).unwrap().client().unwrap();
    assert!(client.checked_full_key(&sdk, Operation::Kv, "oversized", 0, 1).is_err());
    assert!(observation.completions.lock().is_empty());
    client.mark_retired();
    assert!(observation.retired.load(Ordering::Relaxed));
}
