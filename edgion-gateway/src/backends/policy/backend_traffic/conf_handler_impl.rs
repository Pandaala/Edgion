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
#[path = "../../../../tests/unit/backends/policy/backend_traffic/conf_handler_impl_test.rs"]
mod tests;
