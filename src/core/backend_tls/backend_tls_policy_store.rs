//! Global store for BackendTLSPolicy resources

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use arc_swap::ArcSwap;
use std::sync::LazyLock;

use crate::types::resources::BackendTLSPolicy;
use crate::core::conf_sync::traits::ConfHandler;

static GLOBAL_BACKEND_TLS_POLICY_STORE: LazyLock<Arc<BackendTLSPolicyStore>> =
    LazyLock::new(|| Arc::new(BackendTLSPolicyStore::new()));

pub fn get_global_backend_tls_policy_store() -> Arc<BackendTLSPolicyStore> {
    GLOBAL_BACKEND_TLS_POLICY_STORE.clone()
}

/// Create a handler for BackendTLSPolicy
pub fn create_backend_tls_policy_handler() -> Box<dyn ConfHandler<BackendTLSPolicy> + Send + Sync> {
    Box::new(get_global_backend_tls_policy_store())
}

/// Type alias for the backend TLS policy map (key: namespace/name)
type BackendTLSPolicyMap = HashMap<String, Arc<BackendTLSPolicy>>;

pub struct BackendTLSPolicyStore {
    policies: ArcSwap<BackendTLSPolicyMap>,
}

impl BackendTLSPolicyStore {
    pub fn new() -> Self {
        Self {
            policies: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Check if a backend TLS policy exists
    pub fn contains(&self, key: &str) -> bool {
        let map = self.policies.load();
        map.contains_key(key)
    }

    /// Get a backend TLS policy by key (namespace/name)
    pub fn get(&self, key: &str) -> Option<Arc<BackendTLSPolicy>> {
        let map = self.policies.load();
        map.get(key).cloned()
    }

    /// Get a backend TLS policy by namespace and name
    pub fn get_by_ns_name(&self, namespace: &str, name: &str) -> Option<Arc<BackendTLSPolicy>> {
        let key = format!("{}/{}", namespace, name);
        self.get(&key)
    }

    /// Replace all backend TLS policies atomically
    pub fn replace_all(&self, policies: HashMap<String, Arc<BackendTLSPolicy>>) {
        self.policies.store(Arc::new(policies));
    }

    /// Update backend TLS policies atomically (clone map + modify + swap)
    pub fn update(&self, add_or_update: HashMap<String, Arc<BackendTLSPolicy>>, remove: &HashSet<String>) {
        let current = self.policies.load();
        let mut new_map = (**current).clone();

        // Add or update policies
        for (key, policy) in add_or_update {
            new_map.insert(key, policy);
        }

        // Remove policies
        for key in remove {
            new_map.remove(key);
        }

        self.policies.store(Arc::new(new_map));
    }

    /// Get all policies
    pub fn get_all(&self) -> Arc<BackendTLSPolicyMap> {
        self.policies.load_full()
    }

    /// Get all policies in a specific namespace
    pub fn get_by_namespace(&self, namespace: &str) -> Vec<Arc<BackendTLSPolicy>> {
        let map = self.policies.load();
        let prefix = format!("{}/", namespace);
        map.iter()
            .filter(|(key, _)| key.starts_with(&prefix))
            .map(|(_, policy)| policy.clone())
            .collect()
    }

    /// Get policies that apply to a given target
    pub fn get_policies_for_target(
        &self,
        group: &str,
        kind: &str,
        name: &str,
        namespace: Option<&str>,
    ) -> Vec<Arc<BackendTLSPolicy>> {
        let map = self.policies.load();
        map.values()
            .filter(|policy| policy.applies_to(group, kind, name, namespace))
            .cloned()
            .collect()
    }

    /// Count of policies
    pub fn count(&self) -> usize {
        self.policies.load().len()
    }
}

impl Default for BackendTLSPolicyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfHandler<BackendTLSPolicy> for Arc<BackendTLSPolicyStore> {
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

impl ConfHandler<BackendTLSPolicy> for &'static BackendTLSPolicyStore {
    fn full_set(&self, data: &HashMap<String, BackendTLSPolicy>) {
        tracing::info!(
            component = "backend_tls_policy_store",
            cnt = data.len(),
            "full set (static ref)"
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
            "partial update (static ref)"
        );

        let mut combined: HashMap<String, Arc<BackendTLSPolicy>> = add
            .into_iter()
            .map(|(k, v)| (k, Arc::new(v)))
            .collect();

        combined.extend(update.into_iter().map(|(k, v)| (k, Arc::new(v))));

        self.update(combined, &remove);
    }
}
