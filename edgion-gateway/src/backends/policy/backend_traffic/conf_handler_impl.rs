//! `ConfHandler` for `EdgionBackendTrafficPolicy` (Gateway side).
//!
//! Receives policies synced from the Controller and applies them through the
//! arbitration store. Logs metadata only (counts + affected-key count) — never
//! resource bodies.

use super::store::{get_backend_traffic_policy_store, BackendTrafficPolicyStore};
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::resources::EdgionBackendTrafficPolicy;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

impl ConfHandler<EdgionBackendTrafficPolicy> for BackendTrafficPolicyStore {
    fn full_set(&self, data: &HashMap<String, Arc<EdgionBackendTrafficPolicy>>) {
        let owned: HashMap<String, Arc<EdgionBackendTrafficPolicy>> =
            data.iter().map(|(k, v)| (k.clone(), Arc::clone(v))).collect();
        let affected = self.full_set_policies(owned);
        tracing::info!(
            component = "backend_traffic_policy",
            policies = data.len(),
            affected_keys = affected,
            "full set"
        );
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<EdgionBackendTrafficPolicy>>,
        update: HashMap<String, Arc<EdgionBackendTrafficPolicy>>,
        remove: HashSet<String>,
    ) {
        if add.is_empty() && update.is_empty() && remove.is_empty() {
            return;
        }
        let (add_cnt, update_cnt, remove_cnt) = (add.len(), update.len(), remove.len());
        let affected = self.partial_update_policies(add, update, remove);
        tracing::info!(
            component = "backend_traffic_policy",
            add = add_cnt,
            update = update_cnt,
            remove = remove_cnt,
            affected_keys = affected,
            "partial update"
        );
    }
}

/// Create the `ConfHandler` for `EdgionBackendTrafficPolicy` resources.
pub fn create_backend_traffic_policy_handler() -> Box<dyn ConfHandler<EdgionBackendTrafficPolicy> + Send + Sync> {
    Box::new(get_backend_traffic_policy_store())
}

#[cfg(test)]
mod tests {
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
                target_refs: vec![PolicyTargetRef {
                    group: String::new(),
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
        assert!(get_hc_config_store().resolve(key_a).is_some());

        // Retarget the same policy to Service "b".
        let mut update = HashMap::new();
        update.insert("bt6h/p".to_string(), policy("p", "b", LoadBalancerType::Ewma));
        store.partial_update(HashMap::new(), update, HashSet::new());

        assert_eq!(store.get_lb_policy(key_a), None, "old target cleared");
        assert!(get_hc_config_store().resolve(key_a).is_none(), "old HC cleared");
        assert_eq!(
            store.get_lb_policy(key_b),
            Some(ParsedLBPolicy::Ewma),
            "new target populated"
        );
        assert!(get_hc_config_store().resolve(key_b).is_some(), "new HC populated");

        // Cleanup shared global stores.
        store.partial_update(HashMap::new(), HashMap::new(), HashSet::from(["bt6h/p".to_string()]));
    }
}
