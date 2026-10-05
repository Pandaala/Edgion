use super::*;
fn config(value: serde_json::Value) -> EtcdClientConfig {
    serde_json::from_value(value).unwrap()
}
#[test]
fn runtime_policy_normalizes_once_and_keeps_bootstrap_deadline_independent() {
    let config = config(
        serde_json::json!({"endpoints": ["http://ETCD:2379/", "http://etcd:2379"], "timeout": {"dial": 5000, "request": 1}, "maxCallSendSize": 32}),
    );
    let policy = EtcdRuntimePolicy::resolve(&config, &CredentialState::NotRequired).unwrap();
    assert_eq!(policy.endpoints.len(), 1);
    assert_eq!(policy.probe_timeout, Duration::from_millis(1));
    assert_eq!(policy.admission.timeout, Duration::from_secs(5));
    assert!(policy.options.check_send_size_lower_bound(32).is_ok());
    assert!(policy.options.check_send_size_lower_bound(33).is_err());
}
#[test]
fn discovery_with_plaintext_seeds_prepares_tls_for_secure_members() {
    let _guard = crate::link_sys_core::outbound_tls::outbound_tls_test_guard();
    let _ = rustls::crypto::ring::default_provider().install_default();
    let config = config(serde_json::json!({"endpoints": ["http://etcd:2379"], "autoSyncInterval": 1}));
    let policy = EtcdRuntimePolicy::resolve(&config, &CredentialState::NotRequired).unwrap();
    assert!(policy.tls.is_some());
    assert!(!policy.require_tls);
    assert_eq!(policy.tls.unwrap().alpn_protocols, vec![b"h2".to_vec()]);
}
#[test]
fn explicit_tls_cannot_silently_rewrite_plaintext_endpoint() {
    let config = config(serde_json::json!({"endpoints": ["http://etcd:2379"], "tls": {"enabled": true}}));
    assert!(EtcdRuntimePolicy::resolve(&config, &CredentialState::NotRequired).is_err());
}
