use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use arc_swap::ArcSwap;
use std::sync::LazyLock;
use pingora_load_balancing::selection::{BackendSelection, Consistent, RoundRobin};
use pingora_load_balancing::Backend;
use crate::core::lb::leastconn::LeastConnection;
use super::discovery_impl::EndpointSliceLoadBalancer;

/// Store for RoundRobin LoadBalancers (primary, always present)
static ROUNDROBIN_STORE: LazyLock<Arc<EpSliceStore<RoundRobin>>> =
    LazyLock::new(|| Arc::new(EpSliceStore::new()));

/// Store for Consistent LoadBalancers (optional)
static CONSISTENT_STORE: LazyLock<Arc<EpSliceStore<Consistent>>> =
    LazyLock::new(|| Arc::new(EpSliceStore::new()));

/// Store for LeastConnection LoadBalancers (optional)
static LEASTCONN_STORE: LazyLock<Arc<EpSliceStore<LeastConnection>>> =
    LazyLock::new(|| Arc::new(EpSliceStore::new()));

pub fn get_roundrobin_store() -> Arc<EpSliceStore<RoundRobin>> {
    ROUNDROBIN_STORE.clone()
}

pub fn get_consistent_store() -> Arc<EpSliceStore<Consistent>> {
    CONSISTENT_STORE.clone()
}

pub fn get_leastconn_store() -> Arc<EpSliceStore<LeastConnection>> {
    LEASTCONN_STORE.clone()
}

/// Generic store for endpoint slice load balancers
pub struct EpSliceStore<S>
where
    S: BackendSelection + 'static,
    S::Iter: pingora_load_balancing::selection::BackendIter,
{
    ep_slices: ArcSwap<HashMap<String, Arc<EndpointSliceLoadBalancer<S>>>>,
}

impl<S> EpSliceStore<S>
where
    S: BackendSelection + 'static,
    S::Iter: pingora_load_balancing::selection::BackendIter,
{
    pub fn new() -> Self {
        Self {
            ep_slices: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Check if an endpoint slice exists
    pub fn contains(&self, key: &str) -> bool {
        let map = self.ep_slices.load();
        map.contains_key(key)
    }

    /// Get an endpoint slice load balancer by key
    pub fn get(&self, key: &str) -> Option<Arc<EndpointSliceLoadBalancer<S>>> {
        let map = self.ep_slices.load();
        map.get(key).cloned()
    }

    /// Get endpoint slice load balancer by service key (namespace/service-name)
    pub fn get_by_service(&self, service_key: &str) -> Option<Arc<EndpointSliceLoadBalancer<S>>> {
        self.get(service_key)
    }

    /// Select a backend peer from the load balancer
    /// 
    /// # Arguments
    /// * `service_key` - The service key (namespace/service-name)
    /// * `hash_key` - Hash key for consistent hashing (use empty slice for round-robin)
    /// * `max_sample` - Maximum number of backends to sample
    /// 
    /// # Returns
    /// * `Some(Backend)` - Selected backend
    /// * `None` - No backend available or service not found
    pub fn select_peer(&self, service_key: &str, hash_key: &[u8], max_sample: usize) -> Option<Backend> {
        let map = self.ep_slices.load();
        let ep_lb = map.get(service_key)?;
        ep_lb.load_balancer().select(hash_key, max_sample)
    }

    /// Execute a function with the endpoint slice load balancer reference
    pub fn with_ep_slice<F, R>(&self, key: &str, f: F) -> Option<R>
    where
        F: FnOnce(&Arc<EndpointSliceLoadBalancer<S>>) -> R,
    {
        let map = self.ep_slices.load();
        map.get(key).map(f)
    }

    /// Replace all endpoint slices atomically
    pub fn replace_all(&self, ep_slices: HashMap<String, Arc<EndpointSliceLoadBalancer<S>>>) {
        self.ep_slices.store(Arc::new(ep_slices));
    }

    /// Update endpoint slices atomically (clone map + modify + swap)
    pub fn update(&self, add_or_update: HashMap<String, Arc<EndpointSliceLoadBalancer<S>>>, remove: &HashSet<String>) {
        let current = self.ep_slices.load();
        let mut new_map = (**current).clone();
        
        for key in remove {
            new_map.remove(key);
        }
        for (key, lb) in add_or_update {
            new_map.insert(key, lb);
        }
        
        self.ep_slices.store(Arc::new(new_map));
    }

    /// Apply modifications to the map and swap atomically
    pub fn apply_modifications<F>(&self, modify: F)
    where
        F: FnOnce(&mut HashMap<String, Arc<EndpointSliceLoadBalancer<S>>>),
    {
        let current = self.ep_slices.load();
        let mut new_map = (**current).clone();
        modify(&mut new_map);
        self.ep_slices.store(Arc::new(new_map));
    }
    
    /// Update EndpointSlice in-place and refresh LoadBalancer
    /// This is more efficient than rebuilding the entire ArcSwap map
    /// 
    /// # Arguments
    /// * `key` - The EndpointSlice key
    /// * `new_endpoint_slice` - The new EndpointSlice data
    /// 
    /// # Returns
    /// * `Ok(())` - Updated successfully
    /// * `Err(msg)` - Update failed or key not found
    pub fn update_in_place_and_refresh_lb(
        &self,
        key: &str,
        new_endpoint_slice: k8s_openapi::api::discovery::v1::EndpointSlice,
    ) -> Result<(), String> {
        let map = self.ep_slices.load();
        let lb = map.get(key).ok_or_else(|| {
            tracing::debug!(key = %key, "Key not found for in-place update");
            format!("Key not found: {}", key)
        })?;
        
        // Update in-place
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
        Ok(())
    }
}
