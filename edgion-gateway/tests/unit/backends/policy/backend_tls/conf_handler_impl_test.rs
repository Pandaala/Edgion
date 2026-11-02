use super::*;
use edgion_resources::resources::backend_tls_policy::*;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

fn create_test_policy(namespace: &str, name: &str, target_name: &str) -> BackendTLSPolicy {
    let creation_timestamp = k8s_openapi::jiff::Timestamp::now();
    BackendTLSPolicy {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            creation_timestamp: Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(creation_timestamp)),
            ..Default::default()
        },
        spec: BackendTLSPolicySpec {
            prepared_ancestor_admission: Some(edgion_resources::resources::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission::Unrestricted),
            prepared_precedence: Some(
                edgion_resources::resources::common::policy_precedence::PreparedPolicyPrecedence {
                    creation_timestamp: Some(creation_timestamp),
                },
            ),
            current_status: None,
            target_refs: vec![BackendTLSPolicyTargetRef {
                group: "".to_string(),
                kind: "Service".to_string(),
                name: target_name.to_string(),
                section_name: None,
            }],
            resolved_target_class: Some(
                edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::Service,
            ),
            validation: BackendTLSPolicyValidation {
                ca_certificate_refs: None,
                hostname: format!("{}.example.com", target_name),
                well_known_ca_certificates: Some(WellKnownCACertificates::System),
                subject_alt_names: None,
            },
            options: None,
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
            use_system_ca: true,
        },
        status: None,
    }
}

#[test]
fn test_full_set() {
    let store = BackendTLSPolicyStore::new();

    let mut data = HashMap::new();
    data.insert(
        "default/policy1".to_string(),
        Arc::new(create_test_policy("default", "policy1", "svc1")),
    );
    data.insert(
        "default/policy2".to_string(),
        Arc::new(create_test_policy("default", "policy2", "svc2")),
    );

    store.full_set(&data);

    assert!(store.contains("default/policy1"));
    assert!(store.contains("default/policy2"));
    assert!(!store.contains("default/policy3"));
}

#[test]
fn test_partial_update_add() {
    let store = BackendTLSPolicyStore::new();

    let mut add = HashMap::new();
    add.insert(
        "default/policy1".to_string(),
        Arc::new(create_test_policy("default", "policy1", "svc1")),
    );

    store.partial_update(add, HashMap::new(), HashSet::new());

    assert!(store.contains("default/policy1"));
}

#[test]
fn test_partial_update_update() {
    let store = BackendTLSPolicyStore::new();

    let mut data = HashMap::new();
    data.insert(
        "default/policy1".to_string(),
        Arc::new(create_test_policy("default", "policy1", "svc1")),
    );
    store.full_set(&data);

    let mut update = HashMap::new();
    update.insert(
        "default/policy1".to_string(),
        Arc::new(create_test_policy("default", "policy1", "svc1")),
    );

    store.partial_update(HashMap::new(), update, HashSet::new());

    assert!(store.contains("default/policy1"));
}

#[test]
fn test_partial_update_remove() {
    let store = BackendTLSPolicyStore::new();

    let mut data = HashMap::new();
    data.insert(
        "default/policy1".to_string(),
        Arc::new(create_test_policy("default", "policy1", "svc1")),
    );
    store.full_set(&data);

    let mut remove = HashSet::new();
    remove.insert("default/policy1".to_string());
    store.partial_update(HashMap::new(), HashMap::new(), remove);

    assert!(!store.contains("default/policy1"));
}

#[test]
fn test_reverse_index_lookup() {
    let store = BackendTLSPolicyStore::new();

    let mut data = HashMap::new();
    data.insert(
        "default/policy1".to_string(),
        Arc::new(create_test_policy("default", "policy1", "svc1")),
    );
    data.insert(
        "default/policy2".to_string(),
        Arc::new(create_test_policy("default", "policy2", "svc1")),
    );

    store.full_set(&data);

    let target = edgion_resources::resources::edgion_backend_traffic_policy::PolicyTargetKey {
        target_kind: edgion_resources::resources::edgion_backend_traffic_policy::PolicyTargetKind::Service,
        namespace: "default".to_string(),
        name: "svc1".to_string(),
    };
    assert_eq!(store.count(), 2);
    assert!(store.snapshot().lookup(&target, None).effective().is_some());
}

#[test]
fn test_handler_via_create_function() {
    let _tls_serial = crate::backends::policy::GLOBAL_TLS_STORE_TEST_LOCK.blocking_lock();
    let handler = create_backend_tls_policy_handler();

    let mut data = HashMap::new();
    data.insert(
        "default/policy1".to_string(),
        Arc::new(create_test_policy("default", "policy1", "svc1")),
    );

    handler.full_set(&data);

    let store = get_global_backend_tls_policy_store();
    assert!(store.contains("default/policy1"));
}
