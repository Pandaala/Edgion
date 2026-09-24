use super::{get_global_backend_tls_policy_store, BackendTLSPolicyStore};
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::resources::BackendTLSPolicy;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

impl ConfHandler<BackendTLSPolicy> for BackendTLSPolicyStore {
    fn full_set(&self, data: &HashMap<String, Arc<BackendTLSPolicy>>) {
        tracing::info!(component = "backend_tls_policy_store", cnt = data.len(), "full set");

        let policies: HashMap<String, Arc<BackendTLSPolicy>> =
            data.iter().map(|(k, v)| (k.clone(), Arc::clone(v))).collect();

        self.replace_all(policies);
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<BackendTLSPolicy>>,
        update: HashMap<String, Arc<BackendTLSPolicy>>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "backend_tls_policy_store",
            add_cnt = add.len(),
            update_cnt = update.len(),
            remove_cnt = remove.len(),
            "partial update"
        );

        self.update(add, update, &remove);
    }
}

/// Create a BackendTLSPolicyStore handler for registration with ConfigClient
pub fn create_backend_tls_policy_handler() -> Box<dyn ConfHandler<BackendTLSPolicy> + Send + Sync> {
    Box::new(get_global_backend_tls_policy_store())
}

#[cfg(test)]
#[path = "../../../../tests/unit/backends/policy/backend_tls/conf_handler_impl_test.rs"]
mod tests;
