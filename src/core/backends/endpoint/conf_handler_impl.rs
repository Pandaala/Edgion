use super::discovery_impl::{EndpointExt, EndpointLoadBalancer};
use super::{
    get_endpoint_consistent_store, get_endpoint_ewma_store, get_endpoint_leastconn_store, get_endpoint_roundrobin_store,
};
use crate::core::conf_sync::traits::ConfHandler;
use crate::core::lb::lb_policy::{get_global_policy_store, LbPolicy};
use crate::types::ResourceMeta;
use k8s_openapi::api::core::v1::Endpoints;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Handler for Endpoints configuration updates
/// Manages multiple stores for different LB algorithms
pub struct EndpointHandler;

impl EndpointHandler {
    pub fn new() -> Self {
        Self
    }

    /// Full set for RoundRobin store (all Endpoints get RoundRobin LB)
    fn full_set_roundrobin(&self, data: &HashMap<String, Endpoints>) {
        let roundrobin_store = get_endpoint_roundrobin_store();
        let roundrobin_map: HashMap<String, Arc<EndpointLoadBalancer<_>>> = data
            .iter()
            .map(|(key, endpoint)| {
                let lb = EndpointLoadBalancer::new(endpoint.clone());
                (key.clone(), lb)
            })
            .collect();
        roundrobin_store.replace_all(roundrobin_map);
    }

    /// Full set for Consistent store (only for services with Consistent policy)
    fn full_set_consistent(&self, data: &HashMap<String, Endpoints>) -> usize {
        let consistent_store = get_endpoint_consistent_store();
        let policy_store = get_global_policy_store();
        let mut consistent_map = HashMap::new();

        for (key, endpoint) in data {
            let service_key = endpoint.key_name();
            let policies = policy_store.get(&service_key);

            if policies.contains(&LbPolicy::Consistent) {
                let lb = EndpointLoadBalancer::new(endpoint.clone());
                consistent_map.insert(key.clone(), lb);
                tracing::debug!(key = %key, "Created Consistent LB");
            }
        }

        let count = consistent_map.len();
        consistent_store.replace_all(consistent_map);
        count
    }

    /// Full set for LeastConnection store (for services with LeastConnection policy)
    fn full_set_leastconn(&self, data: &HashMap<String, Endpoints>) -> usize {
        let leastconn_store = get_endpoint_leastconn_store();
        let policy_store = get_global_policy_store();
        let mut leastconn_map = HashMap::new();

        for (key, endpoint) in data {
            let service_key = endpoint.key_name();
            let policies = policy_store.get(&service_key);

            if policies.contains(&LbPolicy::LeastConnection) {
                let lb = EndpointLoadBalancer::new(endpoint.clone());
                leastconn_map.insert(key.clone(), lb);
                tracing::debug!(key = %key, "Created LeastConnection LB");
            }
        }

        let count = leastconn_map.len();
        leastconn_store.replace_all(leastconn_map);
        count
    }

    /// Full set for EWMA store (for services with EWMA policy)
    fn full_set_ewma(&self, data: &HashMap<String, Endpoints>) -> usize {
        let ewma_store = get_endpoint_ewma_store();
        let policy_store = get_global_policy_store();
        let mut ewma_map = HashMap::new();

        for (key, endpoint) in data {
            let service_key = endpoint.key_name();
            let policies = policy_store.get(&service_key);

            if policies.contains(&LbPolicy::Ewma) {
                let lb = EndpointLoadBalancer::new(endpoint.clone());
                ewma_map.insert(key.clone(), lb);
                tracing::debug!(key = %key, "Created EWMA LB");
            }
        }

        let count = ewma_map.len();
        ewma_store.replace_all(ewma_map);
        count
    }

    /// Partial update for RoundRobin store
    fn partial_update_roundrobin(
        &self,
        add: &HashMap<String, Endpoints>,
        update: &HashMap<String, Endpoints>,
        remove: &HashSet<String>,
    ) {
        let roundrobin_store = get_endpoint_roundrobin_store();

        // Handle updates (in-place)
        for (key, endpoint) in update {
            if let Err(e) = roundrobin_store.update_in_place_and_refresh_lb(key, endpoint.clone()) {
                tracing::error!(key = %key, error = %e, "Failed to update RoundRobin LB");
            }
        }

        // Handle add/remove
        if !add.is_empty() || !remove.is_empty() {
            roundrobin_store.apply_modifications(|map| {
                for key in remove {
                    map.remove(key);
                }
                for (key, endpoint) in add {
                    let lb = EndpointLoadBalancer::new(endpoint.clone());
                    map.insert(key.clone(), lb);
                }
            });
        }
    }

    /// Partial update for Consistent store
    fn partial_update_consistent(
        &self,
        add: &HashMap<String, Endpoints>,
        update: &HashMap<String, Endpoints>,
        remove: &HashSet<String>,
    ) {
        let consistent_store = get_endpoint_consistent_store();
        let policy_store = get_global_policy_store();

        let mut consistent_add = HashMap::new();
        let mut consistent_remove = HashSet::new();

        // Handle removes
        for key in remove {
            consistent_remove.insert(key.clone());
        }

        // Handle adds and updates
        for (key, endpoint) in add.iter().chain(update.iter()) {
            let service_key = endpoint.key_name();
            let policies = policy_store.get(&service_key);

            if policies.contains(&LbPolicy::Consistent) {
                if !consistent_store.contains(key) {
                    let lb = EndpointLoadBalancer::new(endpoint.clone());
                    consistent_add.insert(key.clone(), lb);
                } else if update.contains_key(key) {
                    if let Err(e) = consistent_store.update_in_place_and_refresh_lb(key, endpoint.clone()) {
                        tracing::error!(key = %key, error = %e, "Failed to update Consistent LB");
                    }
                }
            } else {
                consistent_remove.insert(key.clone());
            }
        }

        if !consistent_add.is_empty() || !consistent_remove.is_empty() {
            consistent_store.update(consistent_add, &consistent_remove);
        }
    }

    /// Partial update for LeastConnection store
    fn partial_update_leastconn(
        &self,
        add: &HashMap<String, Endpoints>,
        update: &HashMap<String, Endpoints>,
        remove: &HashSet<String>,
    ) {
        let leastconn_store = get_endpoint_leastconn_store();
        let policy_store = get_global_policy_store();

        let mut leastconn_add = HashMap::new();
        let mut leastconn_remove = HashSet::new();

        // Handle removes
        // Note: Backend state cleanup will be handled by the cleaner task
        // Backends will be filtered out by the selection algorithm if draining
        for key in remove {
            leastconn_remove.insert(key.clone());
        }

        // Handle updates - check for removed backends and mark as draining
        for (key, new_endpoint) in update {
            let service_key = new_endpoint.key_name();
            let policies = policy_store.get(&service_key);

            if policies.contains(&LbPolicy::LeastConnection) {
                // Update the load balancer
                // Note: Backend state management (draining/reactivation) will be handled
                // by monitoring connection counts in the cleaner task
                if let Err(e) = leastconn_store.update_in_place_and_refresh_lb(key, new_endpoint.clone()) {
                    tracing::error!(key = %key, error = %e, "Failed to update LeastConnection LB");
                }
            } else {
                leastconn_remove.insert(key.clone());
            }
        }

        // Handle adds
        for (key, endpoint) in add {
            let service_key = endpoint.key_name();
            let policies = policy_store.get(&service_key);

            if policies.contains(&LbPolicy::LeastConnection) {
                let lb = EndpointLoadBalancer::new(endpoint.clone());
                leastconn_add.insert(key.clone(), lb);
            }
        }

        if !leastconn_add.is_empty() || !leastconn_remove.is_empty() {
            leastconn_store.update(leastconn_add, &leastconn_remove);
        }
    }

    /// Partial update for EWMA store
    fn partial_update_ewma(
        &self,
        add: &HashMap<String, Endpoints>,
        update: &HashMap<String, Endpoints>,
        remove: &HashSet<String>,
    ) {
        let ewma_store = get_endpoint_ewma_store();
        let policy_store = get_global_policy_store();

        let mut ewma_add = HashMap::new();
        let mut ewma_remove = HashSet::new();

        // Handle removes
        // EWMA metrics for removed backends will be cleaned up
        for key in remove {
            ewma_remove.insert(key.clone());
        }

        // Handle updates
        for (key, new_endpoint) in update {
            let service_key = new_endpoint.key_name();
            let policies = policy_store.get(&service_key);

            if policies.contains(&LbPolicy::Ewma) {
                // Update the load balancer
                if let Err(e) = ewma_store.update_in_place_and_refresh_lb(key, new_endpoint.clone()) {
                    tracing::error!(key = %key, error = %e, "Failed to update EWMA LB");
                }
            } else {
                ewma_remove.insert(key.clone());
            }
        }

        // Handle adds
        for (key, endpoint) in add {
            let service_key = endpoint.key_name();
            let policies = policy_store.get(&service_key);

            if policies.contains(&LbPolicy::Ewma) {
                let lb = EndpointLoadBalancer::new(endpoint.clone());
                ewma_add.insert(key.clone(), lb);
            }
        }

        if !ewma_add.is_empty() || !ewma_remove.is_empty() {
            ewma_store.update(ewma_add, &ewma_remove);
        }
    }
}

/// Create an EndpointStore handler for registration with ConfigClient
pub fn create_endpoint_handler() -> Box<dyn ConfHandler<Endpoints> + Send + Sync> {
    Box::new(EndpointHandler::new())
}

impl ConfHandler<Endpoints> for EndpointHandler {
    fn full_set(&self, data: &HashMap<String, Endpoints>) {
        tracing::info!(component = "endpoint_handler", cnt = data.len(), "full set");

        // 1. RoundRobin for all
        self.full_set_roundrobin(data);

        // 2. Consistent for services with Consistent policy
        let consistent_count = self.full_set_consistent(data);

        // 3. LeastConnection for services with LeastConnection policy
        let leastconn_count = self.full_set_leastconn(data);

        // 4. EWMA for services with EWMA policy
        let ewma_count = self.full_set_ewma(data);

        tracing::info!(
            component = "endpoint_handler",
            total = data.len(),
            consistent = consistent_count,
            leastconn = leastconn_count,
            ewma = ewma_count,
            "Full set completed"
        );
    }

    fn partial_update(
        &self,
        add: HashMap<String, Endpoints>,
        update: HashMap<String, Endpoints>,
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
            component = "endpoint_handler",
            add_count = add_count,
            update_count = update_count,
            remove_count = remove_count,
            "Partial update completed"
        );
    }
}
