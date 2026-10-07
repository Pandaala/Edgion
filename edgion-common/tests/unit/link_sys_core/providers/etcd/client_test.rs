use std::sync::atomic::AtomicUsize;

use super::*;
use edgion_resources::resources::link_sys::etcd::EtcdClientConfig;
use edgion_resources::resources::link_sys::LinkSysSpec;

fn resource_client(name: &str, config: &EtcdClientConfig) -> Result<EtcdLinkClient> {
    let (namespace, name) = name
        .split_once('/')
        .map_or((None, name), |(ns, name)| (Some(ns.to_owned()), name));
    EtcdLinkClient::from_link_sys(Arc::new(LinkSys {
        metadata: kube::core::ObjectMeta {
            name: Some(name.into()),
            namespace,
            ..Default::default()
        },
        spec: LinkSysSpec {
            config: SystemConfig::Etcd(config.clone()),
            current_status: None,
            unknown_fields: vec![],
        },
        status: None,
    }))
}

fn bootstrap_client(name: &str) -> Result<EtcdLinkClient> {
    EtcdLinkClient::from_bootstrap(
        name,
        EtcdBootstrapConfig {
            endpoints: vec!["http://localhost:2379".into()],
            auth: None,
            tls: None,
            timeout: None,
            keep_alive: None,
            namespace: None,
        },
    )
}

impl EtcdLinkClient {
    pub fn assert_retained_connection_input(&self, prepared: &Arc<PreparedLinkSysTls>, actual: &LinkSys) {
        let owner = self.connection._owner.as_ref().expect("resource owner retained");
        assert!(Arc::ptr_eq(owner, prepared));
        assert!(std::ptr::eq(owner.owner(), actual));
        // Reconnect, probe, and election all share this one immutable input.
        let recovered = Arc::clone(&self.connection);
        assert!(Arc::ptr_eq(&recovered, &self.connection));
    }

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
    }
}

#[test]
fn test_from_config_minimal() {
    let config = minimal_config();
    let client = resource_client("default/etcd-test", &config);
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
    let client = resource_client("default/etcd-ns", &config).unwrap();
    assert_eq!(client.namespace(), Some("/app/"));
    assert_eq!(client.full_key("mykey"), "/app/mykey");
}

#[test]
fn test_from_config_without_namespace() {
    let config = minimal_config();
    let client = resource_client("default/etcd-test", &config).unwrap();
    assert_eq!(client.full_key("mykey"), "mykey");
}

#[test]
fn test_empty_endpoints_returns_error() {
    let config = EtcdClientConfig {
        endpoints: vec![],
        ..minimal_config()
    };
    let result = resource_client("test", &config);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("no endpoints"));
}

/// An actual owner with declared unresolved authentication fails before dial.
#[test]
fn test_from_config_pending_state_fails_fast() {
    let mut config = minimal_config();
    config.auth = Some(edgion_resources::resources::link_sys::etcd::EtcdAuth {
        slot: edgion_resources::resources::common::SecretSlot {
            secret_ref: edgion_resources::resources::common::SecretObjectReference {
                group: None,
                kind: None,
                namespace: None,
                name: "auth".into(),
            },
            secret: None,
        },
    });
    let result = resource_client("default/etcd-test", &config);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("non-buildable"));
}

#[test]
fn controller_construction_does_not_enable_endpoint_coverage_sampling() {
    let client = bootstrap_client("default/etcd-test").unwrap();
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
    let client = resource_client("default/etcd-gw", &minimal_config())
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
    let client = resource_client("default/etcd-health", &minimal_config())
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
    // Options must fail before any endpoint is dialed.
    let Err(error) = resource_client("default/etcd-test", &config) else {
        panic!("invalid TLS options must prevent connection");
    };
    assert!(error.to_string().contains("OutboundTlsVerifyUnsupported"));
}

#[tokio::test]
async fn retired_client_cannot_initialize_or_install_a_monitor() {
    let client = resource_client("default/retired", &minimal_config()).unwrap();
    client.mark_retired();
    assert!(client.init().await.unwrap_err().to_string().contains("retired"));
    client.start_health_monitor();
    assert!(client.monitor_handle.lock().is_none());
    assert!(client.get_client().await.is_none());
}

#[test]
fn connection_input_retains_captured_actual_owner_without_rereading_defaults() {
    use crate::link_sys_core::outbound_tls::{test_util, GlobalOutboundTlsSnapshot};
    use edgion_resources::resources::common::OutboundTlsConfig;
    let config = minimal_config();
    let actual = Arc::new(LinkSys {
        metadata: kube::core::ObjectMeta {
            name: Some("retained".into()),
            namespace: Some("apps".into()),
            ..Default::default()
        },
        spec: LinkSysSpec {
            config: SystemConfig::Etcd(config),
            current_status: None,
            unknown_fields: vec![],
        },
        status: None,
    });
    let prepared = Arc::new(
        PreparedLinkSysTls::with_snapshot(actual.clone(), Arc::new(GlobalOutboundTlsSnapshot::from_configs([])))
            .unwrap(),
    );
    let first = EtcdLinkClient::from_prepared_link_sys(prepared.clone()).unwrap();
    let second = EtcdLinkClient::from_prepared_link_sys(prepared.clone()).unwrap();
    first.assert_retained_connection_input(&prepared, &actual);
    second.assert_retained_connection_input(&prepared, &actual);
    // Absent TLS does not consume inherited verify=false, even on HTTPS.
    let mut https = (*actual).clone();
    let SystemConfig::Etcd(config) = &mut https.spec.config else {
        unreachable!()
    };
    config.endpoints = vec!["https://etcd.example:2379".into()];
    let input = Arc::new(
        PreparedLinkSysTls::with_snapshot(
            Arc::new(https),
            test_util::snapshot(OutboundTlsConfig {
                verify: false,
                ..Default::default()
            }),
        )
        .unwrap(),
    );
    assert!(EtcdLinkClient::from_prepared_link_sys(input).is_ok());
}
