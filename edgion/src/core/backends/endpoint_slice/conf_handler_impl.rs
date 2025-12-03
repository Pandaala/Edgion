use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use pingora_load_balancing::selection::RoundRobin;
use crate::core::conf_sync::traits::ConfHandler;
use super::{get_roundrobin_store, get_consistent_store, get_random_store, EpSliceStore};
use super::discovery_impl::EndpointSliceLoadBalancer;
use crate::core::lb::lb_policy::{get_global_policy_store, LbPolicy};
use crate::types::ResourceMeta;

/// Handler for EndpointSlice configuration updates
/// Manages multiple stores for different LB algorithms
pub struct EpSliceHandler;

impl EpSliceHandler {
    pub fn new() -> Self {
        Self
    }
    
    /// Full set for RoundRobin store (all EndpointSlices get RoundRobin LB)
    fn full_set_roundrobin(&self, data: &HashMap<String, EndpointSlice>) -> HashMap<String, Arc<EndpointSliceLoadBalancer<RoundRobin>>> {
        let roundrobin_store = get_roundrobin_store();
        
        let roundrobin_map: HashMap<String, Arc<EndpointSliceLoadBalancer<_>>> = data
            .iter()
            .map(|(key, ep_slice)| {
                let lb = EndpointSliceLoadBalancer::new(ep_slice.clone());
                (key.clone(), lb)
            })
            .collect();
        
        roundrobin_store.replace_all(roundrobin_map.clone());
        roundrobin_map
    }
    
    /// Full set for Consistent store (only for services with Consistent policy)
    fn full_set_consistent(
        &self,
        data: &HashMap<String, EndpointSlice>,
        roundrobin_map: &HashMap<String, Arc<EndpointSliceLoadBalancer<RoundRobin>>>,
    ) -> usize {
        let consistent_store = get_consistent_store();
        let policy_store = get_global_policy_store();
        let mut consistent_map = HashMap::new();
        
        for (key, ep_slice) in data {
            let service_key = ep_slice.key_name();
            let policies = policy_store.get(&service_key);
            
            if !policies.contains(&LbPolicy::Consistent) {
                continue;
            }
            
            if let Some(rr_lb) = roundrobin_map.get(key) {
                let discovery = rr_lb.discovery().clone();
                let lb = EndpointSliceLoadBalancer::new_from_discovery(discovery);
                consistent_map.insert(key.clone(), lb);
                tracing::debug!(key = %key, "Created Consistent LB");
            }
        }
        
        let count = consistent_map.len();
        consistent_store.replace_all(consistent_map);
        count
    }
    
    /// Full set for Random store (for services with LeastConnection policy)
    fn full_set_random(
        &self,
        data: &HashMap<String, EndpointSlice>,
        roundrobin_map: &HashMap<String, Arc<EndpointSliceLoadBalancer<RoundRobin>>>,
    ) -> usize {
        let random_store = get_random_store();
        let policy_store = get_global_policy_store();
        let mut random_map = HashMap::new();
        
        for (key, ep_slice) in data {
            let service_key = ep_slice.key_name();
            let policies = policy_store.get(&service_key);
            
            let needs_random = policies.contains(&LbPolicy::LeastConnection);
            if !needs_random {
                continue;
            }
            
            if let Some(rr_lb) = roundrobin_map.get(key) {
                let discovery = rr_lb.discovery().clone();
                let lb = EndpointSliceLoadBalancer::new_from_discovery(discovery);
                random_map.insert(key.clone(), lb);
                tracing::debug!(key = %key, "Created Random LB");
            }
        }
        
        let count = random_map.len();
        random_store.replace_all(random_map);
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
        
        // Handle updates (in-place)
        for (key, ep_slice) in update {
            if let Err(e) = roundrobin_store.update_in_place_and_refresh_lb(key, ep_slice.clone()) {
                tracing::error!(key = %key, error = %e, "Failed to update RoundRobin LB");
            }
        }
        
        // Handle add/remove
        if !add.is_empty() || !remove.is_empty() {
            roundrobin_store.apply_modifications(|map| {
                for key in remove {
                    map.remove(key);
                }
                for (key, ep_slice) in add {
                    let lb = EndpointSliceLoadBalancer::new(ep_slice.clone());
                    map.insert(key.clone(), lb);
                }
            });
        }
    }
    
    /// Partial update for Consistent store
    fn partial_update_consistent(
        &self,
        add: &HashMap<String, EndpointSlice>,
        update: &HashMap<String, EndpointSlice>,
        remove: &HashSet<String>,
        roundrobin_store: &Arc<EpSliceStore<RoundRobin>>,
    ) {
        let consistent_store = get_consistent_store();
        let policy_store = get_global_policy_store();
        
        let mut consistent_add = HashMap::new();
        let mut consistent_remove = HashSet::new();
        
        // Handle removes
        for key in remove {
            consistent_remove.insert(key.clone());
        }
        
        // Handle adds and updates
        for (key, ep_slice) in add.iter().chain(update.iter()) {
            let service_key = ep_slice.key_name();
            let policies = policy_store.get(&service_key);
            
            if policies.contains(&LbPolicy::Consistent) {
                if !consistent_store.contains(key) {
                    // Need to add
                    if let Some(rr_lb) = roundrobin_store.get(key) {
                        let discovery = rr_lb.discovery().clone();
                        let lb = EndpointSliceLoadBalancer::new_from_discovery(discovery);
                        consistent_add.insert(key.clone(), lb);
                    }
                } else if update.contains_key(key) {
                    // Update existing
                    if let Err(e) = consistent_store.update_in_place_and_refresh_lb(key, ep_slice.clone()) {
                        tracing::error!(key = %key, error = %e, "Failed to update Consistent LB");
                    }
                }
            } else {
                // Policy removed
                consistent_remove.insert(key.clone());
            }
        }
        
        if !consistent_add.is_empty() || !consistent_remove.is_empty() {
            consistent_store.update(consistent_add, &consistent_remove);
        }
    }
    
    /// Partial update for Random store
    fn partial_update_random(
        &self,
        add: &HashMap<String, EndpointSlice>,
        update: &HashMap<String, EndpointSlice>,
        remove: &HashSet<String>,
        roundrobin_store: &Arc<EpSliceStore<RoundRobin>>,
    ) {
        let random_store = get_random_store();
        let policy_store = get_global_policy_store();
        
        let mut random_add = HashMap::new();
        let mut random_remove = HashSet::new();
        
        // Handle removes
        for key in remove {
            random_remove.insert(key.clone());
        }
        
        // Handle adds and updates
        for (key, ep_slice) in add.iter().chain(update.iter()) {
            let service_key = ep_slice.key_name();
            let policies = policy_store.get(&service_key);
            
            let needs_random = policies.contains(&LbPolicy::LeastConnection);
            
            if needs_random {
                if !random_store.contains(key) {
                    // Need to add
                    if let Some(rr_lb) = roundrobin_store.get(key) {
                        let discovery = rr_lb.discovery().clone();
                        let lb = EndpointSliceLoadBalancer::new_from_discovery(discovery);
                        random_add.insert(key.clone(), lb);
                    }
                } else if update.contains_key(key) {
                    // Update existing
                    if let Err(e) = random_store.update_in_place_and_refresh_lb(key, ep_slice.clone()) {
                        tracing::error!(key = %key, error = %e, "Failed to update Random LB");
                    }
                }
            } else {
                // Policy removed
                random_remove.insert(key.clone());
            }
        }
        
        if !random_add.is_empty() || !random_remove.is_empty() {
            random_store.update(random_add, &random_remove);
        }
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
        let roundrobin_map = self.full_set_roundrobin(data);
        
        // 2. Consistent for services with Consistent policy
        let consistent_count = self.full_set_consistent(data, &roundrobin_map);
        
        // 3. Random for services with LeastConnection policy
        let random_count = self.full_set_random(data, &roundrobin_map);
        
        tracing::info!(
            component = "ep_slice_handler",
            total = data.len(),
            consistent = consistent_count,
            random = random_count,
            "Full set completed"
        );
    }

    fn partial_update(&self, add: HashMap<String, EndpointSlice>, update: HashMap<String, EndpointSlice>, remove: HashSet<String>) {
        let add_count = add.len();
        let update_count = update.len();
        let remove_count = remove.len();
        
        let roundrobin_store = get_roundrobin_store();
        
        // 1. RoundRobin store
        self.partial_update_roundrobin(&add, &update, &remove);
        
        // 2. Consistent store
        self.partial_update_consistent(&add, &update, &remove, &roundrobin_store);
        
        // 3. Random store
        self.partial_update_random(&add, &update, &remove, &roundrobin_store);
        
        tracing::info!(
            component = "ep_slice_handler",
            add_count = add_count,
            update_count = update_count,
            remove_count = remove_count,
            "Partial update completed"
        );
    }
}
