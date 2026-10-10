use super::*;
use crate::backends::health::check::get_hc_config_store;
use edgion_resources::resources::edgion_backend_traffic_policy::{
    EdgionBackendTrafficPolicySpec, LoadBalancerConfig, LoadBalancerType, PolicyTargetRef,
};
use edgion_resources::resources::health_check::{ActiveHealthCheckConfig, ServiceHealthCheck};
use edgion_resources::resources::lb_policy::ParsedLBPolicy;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, Time};

fn policy(name: &str, svc: &str, lb_type: LoadBalancerType) -> Arc<EdgionBackendTrafficPolicy> {
    Arc::new(EdgionBackendTrafficPolicy {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some("bt6h".to_string()),
            creation_timestamp: Some(Time("2026-01-01T00:00:00Z".parse().expect("rfc3339"))),
            ..Default::default()
        },
        spec: EdgionBackendTrafficPolicySpec {
            current_status: None,
            target_refs: vec![PolicyTargetRef {
                group: Some(String::new()),
                kind: "Service".to_string(),
                name: svc.to_string(),
            }],
            resolved_target_class: Some(
                edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::Service,
            ),
            retry_constraint: None,
            load_balancer: Some(LoadBalancerConfig {
                r#type: lb_type,
                consistent_hash: None,
                degrade_threshold: None,
            }),
            health_check: Some(ServiceHealthCheck {
                active: Some(ActiveHealthCheckConfig::default()),
            }),
            outlier_detection: None,
            upstream_authority: None,
            circuit_breaker: None,
            connection: None,
        },
        status: None,
    })
}

// partial_update that retargets a policy from bt6h/a to bt6h/b must clear
// the old Service key and populate the new one.
#[test]
fn partial_update_retarget_clears_old_and_populates_new() {
    let _guard = super::super::store::GLOBAL_STORE_TEST_LOCK.lock();
    let store = BackendTrafficPolicyStore::new();
    let key_a = "bt6h/a";
    let key_b = "bt6h/b";

    // Initial: policy targets Service "a".
    let mut add = HashMap::new();
    add.insert("bt6h/p".to_string(), policy("p", "a", LoadBalancerType::LeastConn));
    store.partial_update(add, HashMap::new(), HashSet::new());
    assert_eq!(store.get_lb_policy(key_a), Some(ParsedLBPolicy::LeastConn));
    assert!(get_hc_config_store().resolve_policy(key_a).is_some());

    // Retarget the same policy to Service "b".
    let mut update = HashMap::new();
    update.insert("bt6h/p".to_string(), policy("p", "b", LoadBalancerType::Ewma));
    store.partial_update(HashMap::new(), update, HashSet::new());

    assert_eq!(store.get_lb_policy(key_a), None, "old target cleared");
    assert!(get_hc_config_store().resolve_policy(key_a).is_none(), "old HC cleared");
    assert_eq!(
        store.get_lb_policy(key_b),
        Some(ParsedLBPolicy::Ewma),
        "new target populated"
    );
    assert!(
        get_hc_config_store().resolve_policy(key_b).is_some(),
        "new HC populated"
    );

    // Cleanup shared global stores.
    store.partial_update(HashMap::new(), HashMap::new(), HashSet::from(["bt6h/p".to_string()]));
}
