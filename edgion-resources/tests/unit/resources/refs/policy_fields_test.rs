use super::*;
use serde_json::json;

#[test]
fn actual_policy_namespace_and_native_custom_target_contracts_remain_distinct() {
    let mut tls: BackendTLSPolicy = serde_json::from_value(json!({"metadata":{"name":"tls"},"spec":{"targetRefs":[{"group":"","kind":"Service","name":"backend","sectionName":"https"}],"validation":{"hostname":"backend.example","wellKnownCACertificates":"System"}}})).unwrap();
    let mut traffic: EdgionBackendTrafficPolicy = serde_json::from_value(json!({"metadata":{"name":"traffic"},"spec":{"targetRefs":[{"group":"core","kind":"Service","name":"backend"}]}})).unwrap();
    let field = BackendTlsPolicyTargets::from_policy(&tls).fields().next().unwrap();
    assert_eq!(field.canonical_key().unwrap().namespace, "default");
    assert_eq!(field.section_name(), Some("https"));
    assert_eq!(
        BackendTrafficPolicyTargets::from_policy(&traffic)
            .fields()
            .next()
            .unwrap()
            .canonical_key()
            .unwrap(),
        field.canonical_key().unwrap()
    );
    tls.spec.target_refs[0].group = "core".into();
    assert!(BackendTlsPolicyTargets::from_policy(&tls)
        .fields()
        .next()
        .unwrap()
        .canonical_key()
        .is_err());
    tls.spec.target_refs[0].group.clear();
    for namespace in [Some(""), Some("team"), None] {
        tls.metadata.namespace = namespace.map(str::to_owned);
        traffic.metadata.namespace = namespace.map(str::to_owned);
        let native = BackendTlsPolicyTargets::from_policy(&tls)
            .fields()
            .next()
            .unwrap()
            .canonical_key();
        let custom = BackendTrafficPolicyTargets::from_policy(&traffic)
            .fields()
            .next()
            .unwrap()
            .canonical_key();
        assert_eq!(native, custom);
        assert_eq!(native.is_err(), namespace == Some(""));
    }
    traffic.spec.target_refs[0].name = "bad/name".into();
    assert!(BackendTrafficPolicyTargets::from_policy(&traffic)
        .fields()
        .next()
        .unwrap()
        .canonical_key()
        .is_err());
}
