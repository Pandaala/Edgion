use super::{get_consistent_store, get_ewma_store, get_leastconn_store, get_roundrobin_store};
use crate::core::conf_sync::traits::ConfHandler;
use crate::core::lb::lb_policy::{get_global_policy_store, LbPolicy};
use crate::types::ResourceMeta;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use std::collections::{HashMap, HashSet};

/// Handler for EndpointSlice configuration updates
/// Manages multiple stores for different LB algorithms
pub struct EpSliceHandler;

impl EpSliceHandler {
    pub fn new() -> Self {
        Self
    }

    /// Full set for RoundRobin store (all EndpointSlices get RoundRobin LB)
    fn full_set_roundrobin(&self, data: &HashMap<String, EndpointSlice>) {
        let roundrobin_store = get_roundrobin_store();
        // Store handles aggregation by service_key internally
        roundrobin_store.replace_all(data.clone());
    }

    /// Full set for Consistent store (only for services with Consistent policy)
    fn full_set_consistent(&self, data: &HashMap<String, EndpointSlice>) -> usize {
        let consistent_store = get_consistent_store();
        let policy_store = get_global_policy_store();

        // Filter EndpointSlices by their service's LB policy
        let mut filtered_data = HashMap::new();
        for (key, ep_slice) in data {
            let service_key = ep_slice.key_name();
            let policies = policy_store.get(&service_key);

            if policies.contains(&LbPolicy::Consistent) {
                filtered_data.insert(key.clone(), ep_slice.clone());
                tracing::debug!(key = %key, "Included for Consistent LB");
            }
        }

        let count = filtered_data.len();
        // Store handles aggregation by service_key internally
        consistent_store.replace_all(filtered_data);
        count
    }

    /// Full set for LeastConnection store (for services with LeastConnection policy)
    fn full_set_leastconn(&self, data: &HashMap<String, EndpointSlice>) -> usize {
        let leastconn_store = get_leastconn_store();
        let policy_store = get_global_policy_store();

        // Filter EndpointSlices by their service's LB policy
        let mut filtered_data = HashMap::new();
        for (key, ep_slice) in data {
            let service_key = ep_slice.key_name();
            let policies = policy_store.get(&service_key);

            if policies.contains(&LbPolicy::LeastConnection) {
                filtered_data.insert(key.clone(), ep_slice.clone());
                tracing::debug!(key = %key, "Included for LeastConnection LB");
            }
        }

        let count = filtered_data.len();
        // Store handles aggregation by service_key internally
        leastconn_store.replace_all(filtered_data);
        count
    }

    /// Full set for EWMA store (for services with EWMA policy)
    fn full_set_ewma(&self, data: &HashMap<String, EndpointSlice>) -> usize {
        let ewma_store = get_ewma_store();
        let policy_store = get_global_policy_store();

        // Filter EndpointSlices by their service's LB policy
        let mut filtered_data = HashMap::new();
        for (key, ep_slice) in data {
            let service_key = ep_slice.key_name();
            let policies = policy_store.get(&service_key);

            if policies.contains(&LbPolicy::Ewma) {
                filtered_data.insert(key.clone(), ep_slice.clone());
                tracing::debug!(key = %key, "Included for EWMA LB");
            }
        }

        let count = filtered_data.len();
        // Store handles aggregation by service_key internally
        ewma_store.replace_all(filtered_data);
        count
    }

    /// Partial update for RoundRobin store
    fn partial_update_roundrobin(
        &self,
        add: &HashMap<String, EndpointSlice>,
        update: &HashMap<String, EndpointSlice>,
        remove: &HashSet<String>,
    ) {
        let roundrobin_store = get_roundrobin_store();
        // Use new aggregation-aware update method
        roundrobin_store.update_with_service_aggregation(add.clone(), update.clone(), remove);
    }

    /// Partial update for Consistent store
    fn partial_update_consistent(
        &self,
        add: &HashMap<String, EndpointSlice>,
        update: &HashMap<String, EndpointSlice>,
        remove: &HashSet<String>,
    ) {
        let consistent_store = get_consistent_store();
        let policy_store = get_global_policy_store();

        // Filter by LB policy
        let mut filtered_add = HashMap::new();
        let mut filtered_update = HashMap::new();

        for (key, ep_slice) in add {
            let service_key = ep_slice.key_name();
            if policy_store.get(&service_key).contains(&LbPolicy::Consistent) {
                filtered_add.insert(key.clone(), ep_slice.clone());
            }
        }

        for (key, ep_slice) in update {
            let service_key = ep_slice.key_name();
            if policy_store.get(&service_key).contains(&LbPolicy::Consistent) {
                filtered_update.insert(key.clone(), ep_slice.clone());
            }
        }

        // Use new aggregation-aware update method
        consistent_store.update_with_service_aggregation(filtered_add, filtered_update, remove);
    }

    /// Partial update for LeastConnection store
    fn partial_update_leastconn(
        &self,
        add: &HashMap<String, EndpointSlice>,
        update: &HashMap<String, EndpointSlice>,
        remove: &HashSet<String>,
    ) {
        let leastconn_store = get_leastconn_store();
        let policy_store = get_global_policy_store();

        // Filter by LB policy
        let mut filtered_add = HashMap::new();
        let mut filtered_update = HashMap::new();

        for (key, ep_slice) in add {
            let service_key = ep_slice.key_name();
            if policy_store.get(&service_key).contains(&LbPolicy::LeastConnection) {
                filtered_add.insert(key.clone(), ep_slice.clone());
            }
        }

        for (key, ep_slice) in update {
            let service_key = ep_slice.key_name();
            if policy_store.get(&service_key).contains(&LbPolicy::LeastConnection) {
                filtered_update.insert(key.clone(), ep_slice.clone());
            }
        }

        // Use new aggregation-aware update method
        leastconn_store.update_with_service_aggregation(filtered_add, filtered_update, remove);
    }

    /// Partial update for EWMA store
    fn partial_update_ewma(
        &self,
        add: &HashMap<String, EndpointSlice>,
        update: &HashMap<String, EndpointSlice>,
        remove: &HashSet<String>,
    ) {
        let ewma_store = get_ewma_store();
        let policy_store = get_global_policy_store();

        // Filter by LB policy
        let mut filtered_add = HashMap::new();
        let mut filtered_update = HashMap::new();

        for (key, ep_slice) in add {
            let service_key = ep_slice.key_name();
            if policy_store.get(&service_key).contains(&LbPolicy::Ewma) {
                filtered_add.insert(key.clone(), ep_slice.clone());
            }
        }

        for (key, ep_slice) in update {
            let service_key = ep_slice.key_name();
            if policy_store.get(&service_key).contains(&LbPolicy::Ewma) {
                filtered_update.insert(key.clone(), ep_slice.clone());
            }
        }

        // Use new aggregation-aware update method
        ewma_store.update_with_service_aggregation(filtered_add, filtered_update, remove);
    }
}

/// Create an EpSliceStore handler for registration with ConfigClient
pub fn create_ep_slice_handler() -> Box<dyn ConfHandler<EndpointSlice> + Send + Sync> {
    Box::new(EpSliceHandler::new())
}

impl ConfHandler<EndpointSlice> for EpSliceHandler {
    fn full_set(&self, data: &HashMap<String, EndpointSlice>) {
        tracing::info!(component = "ep_slice_handler", cnt = data.len(), "full set");

        // 1. RoundRobin for all
        self.full_set_roundrobin(data);

        // 2. Consistent for services with Consistent policy
        let consistent_count = self.full_set_consistent(data);

        // 3. LeastConnection for services with LeastConnection policy
        let leastconn_count = self.full_set_leastconn(data);

        // 4. EWMA for services with EWMA policy
        let ewma_count = self.full_set_ewma(data);

        tracing::info!(
            component = "ep_slice_handler",
            total = data.len(),
            consistent = consistent_count,
            leastconn = leastconn_count,
            ewma = ewma_count,
            "Full set completed"
        );
    }

    fn partial_update(
        &self,
        add: HashMap<String, EndpointSlice>,
        update: HashMap<String, EndpointSlice>,
        remove: HashSet<String>,
    ) {
        let add_count = add.len();
        let update_count = update.len();
        let remove_count = remove.len();

        // 1. RoundRobin store
        self.partial_update_roundrobin(&add, &update, &remove);

        // 2. Consistent store
        self.partial_update_consistent(&add, &update, &remove);

        // 3. LeastConnection store
        self.partial_update_leastconn(&add, &update, &remove);

        // 4. EWMA store
        self.partial_update_ewma(&add, &update, &remove);

        tracing::info!(
            component = "ep_slice_handler",
            add_count = add_count,
            update_count = update_count,
            remove_count = remove_count,
            "Partial update completed"
        );
    }
}
