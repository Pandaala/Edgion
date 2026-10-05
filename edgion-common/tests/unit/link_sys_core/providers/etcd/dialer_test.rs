use super::*;
#[test]
fn local_seed_exception_is_limited_to_explicit_local_authorities() {
    let config = serde_json::from_value(serde_json::json!({"endpoints": ["http://127.0.0.1:2379", "http://localhost:2379", "http://example.test:2379"]})).unwrap();
    let policy =
        EtcdRuntimePolicy::resolve(&config, &crate::link_sys_core::credential::CredentialState::NotRequired).unwrap();
    let dialer = EtcdDialer::new(&policy, EgressPolicy::default(), Duration::from_secs(30));
    assert!(dialer
        .local_seeds
        .contains(&"http://127.0.0.1:2379".parse::<http::Uri>().unwrap().to_string()));
    assert!(dialer
        .local_seeds
        .contains(&"http://localhost:2379".parse::<http::Uri>().unwrap().to_string()));
    assert!(!dialer
        .local_seeds
        .contains(&"http://example.test:2379".parse::<http::Uri>().unwrap().to_string()));
    assert!(!dialer
        .local_seeds
        .contains(&"http://127.0.0.1:2380".parse::<http::Uri>().unwrap().to_string()));
}
