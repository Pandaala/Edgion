use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use crate::core::conf_sync::traits::ConfHandler;
use super::{get_roundrobin_store, get_consistent_store, get_random_store};
use super::discovery_impl::EndpointSliceLoadBalancer;
use crate::core::lb::optional_lb::{get_global_policy_store, LbPolicy};
use crate::types::ResourceMeta;

/// Handler for EndpointSlice configuration updates
/// Manages multiple stores for different LB algorithms
pub struct EpSliceHandler;

impl EpSliceHandler {
    pub fn new() -> Self {
        Self
    }
}

/// Create an EpSliceStore handler for registration with ConfigClient
pub fn create_ep_slice_handler() -> Box<dyn ConfHandler<EndpointSlice> + Send + Sync> {
    Box::new(EpSliceHandler::new())
}

impl ConfHandler<EndpointSlice> for EpSliceHandler {
    fn full_set(&self, data: &HashMap<String, EndpointSlice>) {
        tracing::info!(component = "ep_slice_handler", cnt = data.len(), "full set");
        
        let roundrobin_store = get_roundrobin_store();
        let consistent_store = get_consistent_store();
        let random_store = get_random_store();
        let policy_store = get_global_policy_store();
        
        // 1. Create RoundRobin LBs for all EndpointSlices (default)
        let roundrobin_map: HashMap<String, Arc<EndpointSliceLoadBalancer<_>>> = data
            .iter()
            .map(|(key, ep_slice)| {
                let lb = EndpointSliceLoadBalancer::new(ep_slice.clone());
                (key.clone(), lb)
            })
            .collect();
        
        roundrobin_store.replace_all(roundrobin_map.clone());
        
        // 2. Create optional LBs based on policies
        let mut consistent_map = HashMap::new();
        let mut random_map = HashMap::new();
        
        for (key, ep_slice) in data {
            let service_key = ep_slice.key_name();
            let policies = policy_store.get(&service_key);
            
            if policies.is_empty() {
                continue;
            }
            
            // Get the discovery from RoundRobin LB to reuse
            if let Some(rr_lb) = roundrobin_map.get(key) {
                let discovery = rr_lb.discovery().clone();
                
                for policy in policies {
                    match policy {
                        LbPolicy::Consistent => {
                            if !consistent_map.contains_key(key) {
                                let lb = EndpointSliceLoadBalancer::new_from_discovery(discovery.clone());
                                consistent_map.insert(key.clone(), lb);
                                tracing::debug!(key = %key, "Created Consistent LB");
                            }
                        }
                        LbPolicy::FnvHash | LbPolicy::LeastConnection => {
                            if !random_map.contains_key(key) {
                                let lb = EndpointSliceLoadBalancer::new_from_discovery(discovery.clone());
                                random_map.insert(key.clone(), lb);
                                tracing::debug!(key = %key, policy = ?policy, "Created Random LB");
                            }
                        }
                    }
                }
            }
        }
        
        let consistent_count = consistent_map.len();
        let random_count = random_map.len();
        
        consistent_store.replace_all(consistent_map);
        random_store.replace_all(random_map);
        
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
        let consistent_store = get_consistent_store();
        let random_store = get_random_store();
        let policy_store = get_global_policy_store();
        
        // 1. Handle updates for RoundRobin store (in-place)
        for (key, ep_slice) in &update {
            if let Err(e) = roundrobin_store.update_in_place_and_refresh_lb(key, ep_slice.clone()) {
                tracing::error!(key = %key, error = %e, "Failed to update RoundRobin LB");
            }
        }
        
        // 2. Handle add/remove for RoundRobin store
        if !add.is_empty() || !remove.is_empty() {
            roundrobin_store.apply_modifications(|map| {
                for key in &remove {
                    map.remove(key);
                }
                for (key, ep_slice) in &add {
                    let lb = EndpointSliceLoadBalancer::new(ep_slice.clone());
                    map.insert(key.clone(), lb);
                }
            });
        }
        
        // 3. Update optional stores based on policies
        // Collect all affected keys (add + update + remove)
        let mut all_keys: HashSet<String> = HashSet::new();
        all_keys.extend(add.keys().cloned());
        all_keys.extend(update.keys().cloned());
        all_keys.extend(remove.iter().cloned());
        
        let mut consistent_add = HashMap::new();
        let mut consistent_remove = HashSet::new();
        let mut random_add = HashMap::new();
        let mut random_remove = HashSet::new();
        
        for key in &all_keys {
            // If removed, remove from all optional stores
            if remove.contains(key) {
                consistent_remove.insert(key.clone());
                random_remove.insert(key.clone());
                continue;
            }
            
            // Get the EndpointSlice (from add or update)
            let ep_slice = add.get(key).or_else(|| update.get(key));
            if ep_slice.is_none() {
                continue;
            }
            let ep_slice = ep_slice.unwrap();
            
            let service_key = ep_slice.key_name();
            let policies = policy_store.get(&service_key);
            
            if policies.is_empty() {
                // No policies, remove from optional stores if exists
                consistent_remove.insert(key.clone());
                random_remove.insert(key.clone());
                continue;
            }
            
            // Get discovery from RoundRobin store
            let discovery = if let Some(rr_lb) = roundrobin_store.get(key) {
                rr_lb.discovery().clone()
            } else {
                tracing::warn!(key = %key, "RoundRobin LB not found, skipping optional LB creation");
                continue;
            };
            
            let mut needs_consistent = false;
            let mut needs_random = false;
            
            for policy in policies {
                match policy {
                    LbPolicy::Consistent => needs_consistent = true,
                    LbPolicy::FnvHash | LbPolicy::LeastConnection => needs_random = true,
                }
            }
            
            // Handle Consistent store
            if needs_consistent {
                if !consistent_store.contains(key) {
                    let lb = EndpointSliceLoadBalancer::new_from_discovery(discovery.clone());
                    consistent_add.insert(key.clone(), lb);
                } else if update.contains_key(key) {
                    // Update existing Consistent LB
                    if let Err(e) = consistent_store.update_in_place_and_refresh_lb(key, ep_slice.clone()) {
                        tracing::error!(key = %key, error = %e, "Failed to update Consistent LB");
                    }
                }
            } else {
                // Policy removed, remove from store
                consistent_remove.insert(key.clone());
            }
            
            // Handle Random store
            if needs_random {
                if !random_store.contains(key) {
                    let lb = EndpointSliceLoadBalancer::new_from_discovery(discovery.clone());
                    random_add.insert(key.clone(), lb);
                } else if update.contains_key(key) {
                    // Update existing Random LB
                    if let Err(e) = random_store.update_in_place_and_refresh_lb(key, ep_slice.clone()) {
                        tracing::error!(key = %key, error = %e, "Failed to update Random LB");
                    }
                }
            } else {
                // Policy removed, remove from store
                random_remove.insert(key.clone());
            }
        }
        
        // 4. Apply changes to optional stores
        if !consistent_add.is_empty() || !consistent_remove.is_empty() {
            consistent_store.update(consistent_add, &consistent_remove);
        }
        
        if !random_add.is_empty() || !random_remove.is_empty() {
            random_store.update(random_add, &random_remove);
        }
        
        // Log summary
        tracing::info!(
            component = "ep_slice_handler",
            add_count = add_count,
            update_count = update_count,
            remove_count = remove_count,
            "Partial update completed"
        );
    }
}
