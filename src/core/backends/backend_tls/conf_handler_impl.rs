use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use crate::core::conf_sync::traits::ConfHandler;
use crate::types::resources::BackendTLSPolicy;
use super::{BackendTLSPolicyStore, get_global_backend_tls_policy_store};

/// Implement ConfHandler for Arc<BackendTLSPolicyStore>
impl ConfHandler<BackendTLSPolicy> for Arc<BackendTLSPolicyStore> {
    fn full_set(&self, data: &HashMap<String, BackendTLSPolicy>) {
        (**self).full_set(data)
    }

    fn partial_update(
        &self,
        add: HashMap<String, BackendTLSPolicy>,
        update: HashMap<String, BackendTLSPolicy>,
        remove: HashSet<String>,
    ) {
        (**self).partial_update(add, update, remove)
    }
}

/// Implement ConfHandler directly for BackendTLSPolicyStore
impl ConfHandler<BackendTLSPolicy> for BackendTLSPolicyStore {
    fn full_set(&self, data: &HashMap<String, BackendTLSPolicy>) {
        tracing::info!(
            component = "backend_tls_policy_store",
            cnt = data.len(),
            "full set"
        );

        let policies: HashMap<String, Arc<BackendTLSPolicy>> = data
            .iter()
            .map(|(k, v)| (k.clone(), Arc::new(v.clone())))
            .collect();

        self.replace_all(policies);
    }

    fn partial_update(
        &self,
        add: HashMap<String, BackendTLSPolicy>,
        update: HashMap<String, BackendTLSPolicy>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "backend_tls_policy_store",
            add_cnt = add.len(),
            update_cnt = update.len(),
            remove_cnt = remove.len(),
            "partial update"
        );

        let mut combined: HashMap<String, Arc<BackendTLSPolicy>> = add
            .into_iter()
            .map(|(k, v)| (k, Arc::new(v)))
            .collect();

        combined.extend(update.into_iter().map(|(k, v)| (k, Arc::new(v))));

        self.update(combined, &remove);
    }
}

/// Create a BackendTLSPolicyStore handler for registration with ConfigClient
pub fn create_backend_tls_policy_handler() -> Box<dyn ConfHandler<BackendTLSPolicy> + Send + Sync> {
    Box::new(get_global_backend_tls_policy_store())
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use crate::types::resources::backend_tls_policy::*;

    fn create_test_policy(namespace: &str, name: &str, target_name: &str) -> BackendTLSPolicy {
        BackendTLSPolicy {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(namespace.to_string()),
                creation_timestamp: Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
                    chrono::Utc::now()
                )),
                ..Default::default()
            },
            spec: BackendTLSPolicySpec {
                target_refs: vec![BackendTLSPolicyTargetRef {
                    group: "".to_string(),
                    kind: "Service".to_string(),
                    name: target_name.to_string(),
                    namespace: None,
                    section_name: None,
                }],
                validation: BackendTLSPolicyValidation {
                    ca_certificate_refs: None,
                    hostname: format!("{}.example.com", target_name),
                    well_known_ca_certificates: None,
                    subject_alt_names: None,
                },
                options: None,
            },
        }
    }

    #[test]
    fn test_full_set() {
        let store = BackendTLSPolicyStore::new();
        
        let mut data = HashMap::new();
        data.insert("default/policy1".to_string(), create_test_policy("default", "policy1", "svc1"));
        data.insert("default/policy2".to_string(), create_test_policy("default", "policy2", "svc2"));
        
        store.full_set(&data);
        
        assert!(store.contains("default/policy1"));
        assert!(store.contains("default/policy2"));
        assert!(!store.contains("default/policy3"));
    }

    #[test]
    fn test_partial_update_add() {
        let store = BackendTLSPolicyStore::new();
        
        let mut add = HashMap::new();
        add.insert("default/policy1".to_string(), create_test_policy("default", "policy1", "svc1"));
        
        store.partial_update(add, HashMap::new(), HashSet::new());
        
        assert!(store.contains("default/policy1"));
    }
    
    #[test]
    fn test_partial_update_update() {
        let store = BackendTLSPolicyStore::new();
        
        // First add a policy
        let mut data = HashMap::new();
        data.insert("default/policy1".to_string(), create_test_policy("default", "policy1", "svc1"));
        store.full_set(&data);
        
        // Then update it
        let mut update = HashMap::new();
        update.insert("default/policy1".to_string(), create_test_policy("default", "policy1", "svc1"));
        
        store.partial_update(HashMap::new(), update, HashSet::new());
        
        assert!(store.contains("default/policy1"));
    }

    #[test]
    fn test_partial_update_remove() {
        let store = BackendTLSPolicyStore::new();
        
        // First add a policy
        let mut data = HashMap::new();
        data.insert("default/policy1".to_string(), create_test_policy("default", "policy1", "svc1"));
        store.full_set(&data);
        
        // Then remove it
        let mut remove = HashSet::new();
        remove.insert("default/policy1".to_string());
        store.partial_update(HashMap::new(), HashMap::new(), remove);
        
        assert!(!store.contains("default/policy1"));
    }

    #[test]
    fn test_reverse_index_lookup() {
        let store = BackendTLSPolicyStore::new();
        
        let mut data = HashMap::new();
        data.insert("default/policy1".to_string(), create_test_policy("default", "policy1", "svc1"));
        data.insert("default/policy2".to_string(), create_test_policy("default", "policy2", "svc1"));
        
        store.full_set(&data);
        
        // Should find both policies for svc1
        let policies = store.get_policies_for_target("svc1", Some("default"));
        assert_eq!(policies.len(), 2);
    }

    #[test]
    fn test_handler_via_create_function() {
        let handler = create_backend_tls_policy_handler();
        
        let mut data = HashMap::new();
        data.insert(
            "default/policy1".to_string(),
            create_test_policy("default", "policy1", "svc1"),
        );
        
        handler.full_set(&data);
        
        let store = get_global_backend_tls_policy_store();
        assert!(store.contains("default/policy1"));
    }
}

