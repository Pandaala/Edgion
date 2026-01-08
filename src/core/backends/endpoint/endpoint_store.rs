use super::discovery_impl::EndpointLoadBalancer;
use crate::core::lb::ewma::Ewma;
use crate::core::lb::leastconn::LeastConnection;
use arc_swap::ArcSwap;
use pingora_load_balancing::selection::{BackendSelection, Consistent, RoundRobin};
use pingora_load_balancing::Backend;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::LazyLock;

/// Store for RoundRobin LoadBalancers (primary, always present)
static ROUNDROBIN_STORE: LazyLock<Arc<EndpointStore<RoundRobin>>> = LazyLock::new(|| Arc::new(EndpointStore::new()));

/// Store for Consistent LoadBalancers (optional)
static CONSISTENT_STORE: LazyLock<Arc<EndpointStore<Consistent>>> = LazyLock::new(|| Arc::new(EndpointStore::new()));

/// Store for LeastConnection LoadBalancers (optional)
static LEASTCONN_STORE: LazyLock<Arc<EndpointStore<LeastConnection>>> =
    LazyLock::new(|| Arc::new(EndpointStore::new()));

/// Store for EWMA LoadBalancers (optional)
static EWMA_STORE: LazyLock<Arc<EndpointStore<Ewma>>> = LazyLock::new(|| Arc::new(EndpointStore::new()));

pub fn get_endpoint_roundrobin_store() -> Arc<EndpointStore<RoundRobin>> {
    ROUNDROBIN_STORE.clone()
}

pub fn get_endpoint_consistent_store() -> Arc<EndpointStore<Consistent>> {
    CONSISTENT_STORE.clone()
}

pub fn get_endpoint_leastconn_store() -> Arc<EndpointStore<LeastConnection>> {
    LEASTCONN_STORE.clone()
}

pub fn get_endpoint_ewma_store() -> Arc<EndpointStore<Ewma>> {
    EWMA_STORE.clone()
}

/// Generic store for endpoint load balancers
pub struct EndpointStore<S>
where
    S: BackendSelection + 'static,
    S::Iter: pingora_load_balancing::selection::BackendIter,
{
    endpoints: ArcSwap<HashMap<String, Arc<EndpointLoadBalancer<S>>>>,
}

impl<S> Default for EndpointStore<S>
where
    S: BackendSelection + 'static,
    S::Iter: pingora_load_balancing::selection::BackendIter,
 {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> EndpointStore<S>
where
    S: BackendSelection + 'static,
    S::Iter: pingora_load_balancing::selection::BackendIter,
{
    pub fn new() -> Self {
        Self {
            endpoints: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Check if an endpoint exists
    pub fn contains(&self, key: &str) -> bool {
        let map = self.endpoints.load();
        map.contains_key(key)
    }

    /// Get an endpoint load balancer by key
    pub fn get(&self, key: &str) -> Option<Arc<EndpointLoadBalancer<S>>> {
        let map = self.endpoints.load();
        map.get(key).cloned()
    }

    /// Get endpoint load balancer by service key (namespace/service-name)
    pub fn get_by_service(&self, service_key: &str) -> Option<Arc<EndpointLoadBalancer<S>>> {
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
        let map = self.endpoints.load();
        let ep_lb = map.get(service_key)?;
        ep_lb.load_balancer().select(hash_key, max_sample)
    }

    /// Execute a function with the endpoint load balancer reference
    pub fn with_endpoint<F, R>(&self, key: &str, f: F) -> Option<R>
    where
        F: FnOnce(&Arc<EndpointLoadBalancer<S>>) -> R,
    {
        let map = self.endpoints.load();
        map.get(key).map(f)
    }

    /// Replace all endpoints atomically
    pub fn replace_all(&self, endpoints: HashMap<String, Arc<EndpointLoadBalancer<S>>>) {
        self.endpoints.store(Arc::new(endpoints));
    }

    /// Update endpoints atomically (clone map + modify + swap)
    pub fn update(&self, add_or_update: HashMap<String, Arc<EndpointLoadBalancer<S>>>, remove: &HashSet<String>) {
        let current = self.endpoints.load();
        let mut new_map = (**current).clone();

        for key in remove {
            new_map.remove(key);
        }
        for (key, lb) in add_or_update {
            new_map.insert(key, lb);
        }

        self.endpoints.store(Arc::new(new_map));
    }

    /// Apply modifications to the map and swap atomically
    pub fn apply_modifications<F>(&self, modify: F)
    where
        F: FnOnce(&mut HashMap<String, Arc<EndpointLoadBalancer<S>>>),
    {
        let current = self.endpoints.load();
        let mut new_map = (**current).clone();
        modify(&mut new_map);
        self.endpoints.store(Arc::new(new_map));
    }

    /// Update Endpoints in-place and refresh LoadBalancer
    /// This is more efficient than rebuilding the entire ArcSwap map
    ///
    /// # Arguments
    /// * `key` - The Endpoints key
    /// * `new_endpoint` - The new Endpoints data
    ///
    /// # Returns
    /// * `Ok(())` - Updated successfully
    /// * `Err(msg)` - Update failed or key not found
    pub fn update_in_place_and_refresh_lb(
        &self,
        key: &str,
        new_endpoint: k8s_openapi::api::core::v1::Endpoints,
    ) -> Result<(), String> {
        let map = self.endpoints.load();
        let lb = map.get(key).ok_or_else(|| {
            tracing::debug!(key = %key, "Key not found for in-place update");
            format!("Key not found: {}", key)
        })?;

        // Update in-place
        if let Err(e) = lb.update(new_endpoint) {
            tracing::error!(key = %key, error = %e, "Failed to update Endpoints data");
            return Err(e);
        }

        // Trigger LoadBalancer update using now_or_never for sync execution
        use futures::FutureExt;
        match lb.update_load_balancer().now_or_never() {
            Some(Ok(_)) => {
                tracing::debug!(key = %key, "Updated Endpoints and LoadBalancer in-place");
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
