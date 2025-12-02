use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use arc_swap::ArcSwap;
use once_cell::sync::Lazy;
use super::discovery_impl::EndpointSliceLoadBalancer;
use crate::types::ResourceMeta;

static GLOBAL_EP_SLICE_STORE: Lazy<Arc<EpSliceStore>> =
    Lazy::new(|| Arc::new(EpSliceStore::new()));

pub fn get_global_ep_slice_store() -> Arc<EpSliceStore> {
    GLOBAL_EP_SLICE_STORE.clone()
}

/// Type alias for the endpoint slice load balancer map
type EpSliceMap = HashMap<String, Arc<EndpointSliceLoadBalancer>>;

pub struct EpSliceStore {
    ep_slices: ArcSwap<Arc<EpSliceMap>>,
}

impl EpSliceStore {
    pub fn new() -> Self {
        Self {
            ep_slices: ArcSwap::from_pointee(Arc::new(HashMap::new())),
        }
    }

    /// Check if an endpoint slice exists
    pub fn contains(&self, key: &str) -> bool {
        let map = self.ep_slices.load();
        map.contains_key(key)
    }

    /// Get an endpoint slice load balancer by key
    pub fn get(&self, key: &str) -> Option<Arc<EndpointSliceLoadBalancer>> {
        let map = self.ep_slices.load();
        map.get(key).cloned()
    }

    /// Get endpoint slice load balancer by service key (namespace/service-name)
    /// This searches for endpoint slices that belong to the given service
    pub fn get_by_service(&self, service_key: &str) -> Option<Arc<EndpointSliceLoadBalancer>> {
        let map = self.ep_slices.load();
        // EndpointSlice key format: "namespace/ep-slice-name"
        // Service key format: "namespace/service-name"
        // We need to find ep_slice with matching service label
        const SERVICE_NAME_LABEL: &str = "kubernetes.io/service-name";
        for (_, ep_slice_lb) in map.iter() {
            let matches = ep_slice_lb.with_endpoint_slice(|ep_slice| {
                let metadata = &ep_slice.metadata;
                let namespace = metadata.namespace.as_deref()?;
                let labels = metadata.labels.as_ref()?;
                let service_name = labels.get(SERVICE_NAME_LABEL)?;
                let key = format!("{}/{}", namespace, service_name);
                Some(key == service_key)
            });
            if matches == Some(true) {
                return Some(Arc::clone(ep_slice_lb));
            }
        }
        None
    }

    /// Execute a function with the endpoint slice load balancer reference
    pub fn with_ep_slice<F, R>(&self, key: &str, f: F) -> Option<R>
    where
        F: FnOnce(&Arc<EndpointSliceLoadBalancer>) -> R,
    {
        let map = self.ep_slices.load();
        map.get(key).map(f)
    }

    /// Replace all endpoint slices atomically
    pub fn replace_all(&self, ep_slices: HashMap<String, Arc<EndpointSliceLoadBalancer>>) {
        self.ep_slices.store(Arc::new(Arc::new(ep_slices)));
    }

    /// Update endpoint slices atomically (clone map + modify + swap)
    pub fn update(&self, add_or_update: HashMap<String, Arc<EndpointSliceLoadBalancer>>, remove: &HashSet<String>) {
        let current = self.ep_slices.load();
        let current_map: &EpSliceMap = &**current;
        let mut new_map: EpSliceMap = current_map.clone();
        
        for key in remove {
            new_map.remove(key);
        }
        for (key, ep_slice_lb) in add_or_update {
            new_map.insert(key, ep_slice_lb);
        }
        
        self.ep_slices.store(Arc::new(Arc::new(new_map)));
    }
    
    /// Apply modifications to the map atomically with a single ArcSwap operation
    /// This is more efficient when you need to do multiple operations at once
    pub fn apply_modifications<F>(&self, f: F)
    where
        F: FnOnce(&mut HashMap<String, Arc<EndpointSliceLoadBalancer>>),
    {
        let current = self.ep_slices.load();
        let current_map: &EpSliceMap = &**current;
        let mut new_map: EpSliceMap = current_map.clone();
        f(&mut new_map);
        self.ep_slices.store(Arc::new(Arc::new(new_map)));
    }
    
    /// Update an existing EndpointSlice in-place without rebuilding the map
    /// Returns Ok(true) if updated, Ok(false) if key not found, Err on update failure
    pub fn update_in_place(&self, key: &str, new_endpoint_slice: k8s_openapi::api::discovery::v1::EndpointSlice) -> Result<bool, String> {
        let map = self.ep_slices.load();
        if let Some(lb) = map.get(key) {
            lb.update(new_endpoint_slice)?;
            tracing::debug!(key = %key, "Updated EndpointSlice in-place");
            Ok(true)
        } else {
            Ok(false)
        }
    }
    
    /// Update EndpointSlice in-place and refresh LoadBalancer
    /// This is more efficient than rebuilding the entire ArcSwap map
    /// 
    /// # Arguments
    /// * `key` - The EndpointSlice key
    /// * `new_endpoint_slice` - The new EndpointSlice data
    /// * `policies` - Optional policies to check for LB rebuild. Empty slice means no rebuild check.
    /// 
    /// # Returns
    /// * `Ok(true)` - Rebuild needed (should be added to rebuild list)
    /// * `Ok(false)` - Updated in-place successfully, no rebuild needed
    /// * `Err(msg)` - Update failed or key not found
    pub fn update_in_place_and_refresh_lb(
        &self,
        key: &str,
        new_endpoint_slice: k8s_openapi::api::discovery::v1::EndpointSlice,
        policies: &[crate::core::lb::optional_lb::LbPolicy],
    ) -> Result<bool, String> {
        let map = self.ep_slices.load();
        let lb = map.get(key).ok_or_else(|| {
            tracing::debug!(key = %key, "Key not found for in-place update");
            format!("Key not found: {}", key)
        })?;
        
        // Check if rebuild is needed (only if policies are provided)
        if !policies.is_empty() {
            let service_key = new_endpoint_slice.key_name();
            if lb.needs_rebuild_for_policies(&service_key, policies) {
                // Need rebuild, return true
                return Ok(true);
            }
        }
        
        // No rebuild needed, update in-place
        if let Err(e) = lb.update(new_endpoint_slice) {
            tracing::error!(key = %key, error = %e, "Failed to update EndpointSlice data");
            return Err(e);
        }
        
        // Trigger LoadBalancer update using now_or_never for sync execution
        use futures::FutureExt;
        match lb.update_load_balancer().now_or_never() {
            Some(Ok(_)) => {
                tracing::debug!(key = %key, "Updated EndpointSlice and LoadBalancer in-place");
            }
            Some(Err(e)) => {
                tracing::warn!(key = %key, error = %e, "Failed to refresh LoadBalancer, will retry on next update");
            }
            None => {
                tracing::error!(key = %key, "LoadBalancer update blocked unexpectedly");
            }
        }
        Ok(false)
    }
}

