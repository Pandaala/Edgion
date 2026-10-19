use super::*;
use crate::backends::discovery::{ServiceDiscoveryCoordinator, ServiceSectionKey};
use crate::backends::BackendTLSPolicyStore;
use crate::plugins::PluginRuntime;
use edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass;
use edgion_resources::resources::BackendTLSPolicy;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

fn backend(port: Option<i32>) -> CompiledBackend {
    let schema = serde_json::from_value(json!({"name": "app", "port": port})).unwrap();
    crate::routes::http::match_unit::compile_http_backend_fixture(schema, "default", 3, Arc::new(PluginRuntime::new()))
}

fn publish(coordinator: &ServiceDiscoveryCoordinator, target_port: i32) {
    coordinator.replace_services(&HashMap::from([(
        "default/app".to_owned(),
        Arc::new(
            serde_json::from_value(json!({
                "metadata": {"name": "app", "namespace": "default"},
                "spec": {"ports": [
                    {
                        "name": "https",
                        "port": 443,
                        "targetPort": target_port,
                        "appProtocol": "kubernetes.io/h2c"
                    },
                    {
                        "name": "admin",
                        "port": 444,
                        "targetPort": 9000,
                        "appProtocol": "kubernetes.io/wss"
                    }
                ]}
            }))
            .unwrap(),
        ),
    )]));
    coordinator.replace_endpoint_slices(&HashMap::from([(
        "default/app-a".to_owned(),
        Arc::new(
            serde_json::from_value(json!({
                "metadata": {"name": "app-a", "namespace": "default", "labels": {"kubernetes.io/service-name": "app"}},
                "addressType": "IPv4",
                "ports": [{"name": "https", "port": target_port}, {"name": "admin", "port": 9000}],
                "endpoints": [{"addresses": ["10.0.0.1"], "conditions": {"ready": false}}]
            }))
            .unwrap(),
        ),
    )]));
}

fn policy_store(valid: bool) -> BackendTLSPolicyStore {
    let mut policy: BackendTLSPolicy = serde_json::from_value(json!({
        "metadata": {"name": "app-tls", "namespace": "default"},
        "spec": {
            "targetRefs": [{"group": "", "kind": "Service", "name": "app", "sectionName": "https"}],
            "validation": {"hostname": "backend.example.com", "wellKnownCACertificates": "System"}
        }
    }))
    .unwrap();
    policy.spec.prepared_precedence = Some(
        edgion_resources::resources::common::policy_precedence::PreparedPolicyPrecedence {
            creation_timestamp: None,
        },
    );
    policy.spec.resolved_target_class = Some(ResolvedPolicyTargetClass::Service);
    policy.spec.use_system_ca = true;
    if !valid {
        policy.spec.validation.hostname = "invalid hostname".to_owned();
    }
    let store = BackendTLSPolicyStore::new();
    store.replace_all(HashMap::from([("default/app-tls".to_owned(), Arc::new(policy))]));
    store
}

#[test]
fn direct_authorization_rejects_invalid_ports_and_denied_refs_before_requests() {
    for port in [None, Some(0), Some(-1), Some(65536), Some(65979)] {
        assert!(backend(port).permitted_service().is_none());
    }
    let mut schema = backend(Some(443)).schema().clone();
    schema.ref_denied = Some(
        serde_json::from_value(json!({
            "targetNamespace": "default", "targetName": "app", "reason": "NoMatchingReferenceGrant"
        }))
        .unwrap(),
    );
    assert!(crate::routes::http::match_unit::compile_http_backend_fixture(
        schema.clone(),
        "default",
        0,
        Arc::new(PluginRuntime::new())
    )
    .permitted_service()
    .is_none());
    schema.ref_denied = None;
    schema.kind = Some("EdgionBackend".to_owned());
    assert!(crate::routes::http::match_unit::compile_http_backend_fixture(
        schema.clone(),
        "default",
        0,
        Arc::new(PluginRuntime::new())
    )
    .permitted_service()
    .is_none());
}

#[test]
fn direct_authorization_is_allocation_free_and_section_scoped() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    publish(&coordinator, 8443);
    let discovery = coordinator.load_generation();
    let policies = policy_store(true).snapshot();
    let backends = [backend(Some(443))];
    let ip = "10.0.0.1".parse().unwrap();
    let (result, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        validate_endpoint_in_route(ip, None, &backends, &discovery, &policies, true)
    });
    let endpoint = result.unwrap();
    assert_eq!(endpoint.addr.port(), 8443);
    assert_eq!(endpoint.backend_ref_idx, 3);
    assert_eq!(endpoint.app_protocol, Some(BackendAppProtocol::H2c));
    let published = discovery
        .available("default/app")
        .unwrap()
        .resolve_section(&ServiceSectionKey::Number(std::num::NonZeroU16::new(443).unwrap()))
        .unwrap()
        .prepared_app_protocol;
    assert_eq!(endpoint.app_protocol, published);
    assert_eq!(
        endpoint.tls_policy.effective().unwrap().policy.spec.validation.hostname,
        "backend.example.com"
    );
    assert_eq!(allocations.calls, 0);
    assert_eq!(allocations.requested_bytes, 0);
    for port in [0, 443, 9000] {
        assert!(validate_endpoint_in_route(ip, Some(port), &backends, &discovery, &policies, true).is_err());
    }
    let explicit = validate_endpoint_in_route(ip, Some(8443), &backends, &discovery, &policies, true).unwrap();
    assert_eq!(explicit.app_protocol, Some(BackendAppProtocol::H2c));

    let adjacent = [backend(Some(444))];
    let inferred_adjacent = validate_endpoint_in_route(ip, None, &adjacent, &discovery, &policies, false).unwrap();
    assert_eq!(inferred_adjacent.addr.port(), 9000);
    assert_eq!(inferred_adjacent.app_protocol, Some(BackendAppProtocol::WebSocket));
    let explicit_adjacent =
        validate_endpoint_in_route(ip, Some(9000), &adjacent, &discovery, &policies, false).unwrap();
    assert_eq!(explicit_adjacent.addr.port(), 9000);
    assert_eq!(explicit_adjacent.app_protocol, Some(BackendAppProtocol::WebSocket));
}

#[test]
fn direct_authorization_pins_mapping_and_policy_and_rejects_missing_generations() {
    let coordinator = ServiceDiscoveryCoordinator::new();
    publish(&coordinator, 8443);
    let old = coordinator.load_generation();
    let policies = policy_store(true).snapshot();
    let backends = [backend(Some(443))];
    let ip = "10.0.0.1".parse().unwrap();
    let captured = validate_endpoint_in_route(ip, None, &backends, &old, &policies, true).unwrap();
    publish(&coordinator, 9443);
    let new = coordinator.load_generation();
    assert_eq!(
        validate_endpoint_in_route(ip, None, &backends, &old, &policies, true)
            .unwrap()
            .addr
            .port(),
        8443
    );
    assert_eq!(
        validate_endpoint_in_route(ip, None, &backends, &new, &policies, true)
            .unwrap()
            .addr
            .port(),
        9443
    );
    assert!(validate_endpoint_in_route(ip, Some(8443), &backends, &new, &policies, true).is_err());
    let invalid = policy_store(false).snapshot();
    assert!(matches!(
        validate_endpoint_in_route(ip, None, &backends, &new, &invalid, true),
        Err(EdgionErrorCode::BackendTlsPolicyInvalid)
    ));
    assert!(validate_endpoint_in_route(ip, None, &backends, &new, &invalid, false)
        .unwrap()
        .tls_policy
        .is_absent());
    assert!(!captured.tls_policy.is_invalid());
    coordinator.update_services(
        HashMap::new(),
        HashMap::new(),
        HashSet::from(["default/app".to_owned()]),
    );
    assert!(validate_endpoint_in_route(ip, None, &backends, &coordinator.load_generation(), &policies, true).is_err());
}
