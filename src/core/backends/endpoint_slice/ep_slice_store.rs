use super::discovery_impl::EndpointSliceLoadBalancer;
use crate::core::lb::ewma::Ewma;
use crate::core::lb::leastconn::LeastConnection;
use arc_swap::ArcSwap;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use pingora_load_balancing::selection::{BackendSelection, Consistent, RoundRobin};
use pingora_load_balancing::Backend;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use std::sync::{Arc, RwLock};

/// Store for RoundRobin LoadBalancers (primary, always present)
static ROUNDROBIN_STORE: LazyLock<Arc<EpSliceStore<RoundRobin>>> = LazyLock::new(|| Arc::new(EpSliceStore::new()));

/// Store for Consistent LoadBalancers (optional)
static CONSISTENT_STORE: LazyLock<Arc<EpSliceStore<Consistent>>> = LazyLock::new(|| Arc::new(EpSliceStore::new()));

/// Store for LeastConnection LoadBalancers (optional)
static LEASTCONN_STORE: LazyLock<Arc<EpSliceStore<LeastConnection>>> = LazyLock::new(|| Arc::new(EpSliceStore::new()));

/// Store for EWMA LoadBalancers (optional)
static EWMA_STORE: LazyLock<Arc<EpSliceStore<Ewma>>> = LazyLock::new(|| Arc::new(EpSliceStore::new()));

pub fn get_roundrobin_store() -> Arc<EpSliceStore<RoundRobin>> {
    ROUNDROBIN_STORE.clone()
}

pub fn get_consistent_store() -> Arc<EpSliceStore<Consistent>> {
    CONSISTENT_STORE.clone()
}

pub fn get_leastconn_store() -> Arc<EpSliceStore<LeastConnection>> {
    LEASTCONN_STORE.clone()
}

pub fn get_ewma_store() -> Arc<EpSliceStore<Ewma>> {
    EWMA_STORE.clone()
}

/// Generic store for endpoint slice load balancers
///
/// This store aggregates multiple EndpointSlices per Service and maintains
/// the mapping from service_key to LoadBalancer.
pub struct EpSliceStore<S>
where
    S: BackendSelection + 'static,
    S::Iter: pingora_load_balancing::selection::BackendIter,
{
    /// Hot path: Service key -> aggregated LoadBalancer
    /// Key: namespace/service-name
    /// Value: LoadBalancer aggregating all EndpointSlices for this Service
    service_lbs: ArcSwap<HashMap<String, Arc<EndpointSliceLoadBalancer<S>>>>,

    /// Cold path: Service key -> EndpointSlice keys
    /// Key: namespace/service-name
    /// Value: Vec<namespace/endpointslice-name>
    service_to_slices: RwLock<HashMap<String, Vec<String>>>,

    /// Cold path: EndpointSlice storage
    /// Key: namespace/endpointslice-name
    /// Value: (EndpointSlice, service_key)
    ep_slices: RwLock<HashMap<String, (EndpointSlice, String)>>,
}

impl<S> Default for EpSliceStore<S>
where
    S: BackendSelection + 'static,
    S::Iter: pingora_load_balancing::selection::BackendIter,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<S> EpSliceStore<S>
where
    S: BackendSelection + 'static,
    S::Iter: pingora_load_balancing::selection::BackendIter,
{
    pub fn new() -> Self {
        Self {
            service_lbs: ArcSwap::from_pointee(HashMap::new()),
            service_to_slices: RwLock::new(HashMap::new()),
            ep_slices: RwLock::new(HashMap::new()),
        }
    }

    /// Check if a service exists
    pub fn contains(&self, service_key: &str) -> bool {
        let map = self.service_lbs.load();
        map.contains_key(service_key)
    }

    /// Get service load balancer by service key
    pub fn get(&self, service_key: &str) -> Option<Arc<EndpointSliceLoadBalancer<S>>> {
        let map = self.service_lbs.load();
        map.get(service_key).cloned()
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
        let map = self.service_lbs.load();
        let ep_lb = map.get(service_key)?;
        ep_lb.load_balancer().select(hash_key, max_sample)
    }

    /// Execute a function with the service load balancer reference
    pub fn with_ep_slice<F, R>(&self, service_key: &str, f: F) -> Option<R>
    where
        F: FnOnce(&Arc<EndpointSliceLoadBalancer<S>>) -> R,
    {
        let map = self.service_lbs.load();
        map.get(service_key).map(f)
    }

    /// Replace all endpoint slices atomically
    /// This aggregates EndpointSlices by service_key
    pub fn replace_all(&self, data: HashMap<String, EndpointSlice>) {
        // 1. Group EndpointSlices by service_key
        let mut service_groups: HashMap<String, Vec<EndpointSlice>> = HashMap::new();
        let mut new_ep_slices: HashMap<String, (EndpointSlice, String)> = HashMap::new();
        let mut new_service_to_slices: HashMap<String, Vec<String>> = HashMap::new();

        for (ep_key, ep_slice) in data {
            let service_key = extract_service_key(&ep_slice);

            service_groups
                .entry(service_key.clone())
                .or_default()
                .push(ep_slice.clone());

            new_ep_slices.insert(ep_key.clone(), (ep_slice, service_key.clone()));
            new_service_to_slices.entry(service_key).or_default().push(ep_key);
        }

        // 2. Create aggregated LoadBalancers per service
        let mut new_service_lbs = HashMap::new();
        for (service_key, slices) in service_groups {
            let lb = EndpointSliceLoadBalancer::new_with_slices(slices);
            new_service_lbs.insert(service_key, lb);
        }

        // 3. Atomically update all stores
        self.service_lbs.store(Arc::new(new_service_lbs));
        *self.service_to_slices.write().unwrap() = new_service_to_slices;
        *self.ep_slices.write().unwrap() = new_ep_slices;
    }

    /// Update endpoint slices with service aggregation
    /// This handles incremental updates and re-aggregates affected services
    pub fn update_with_service_aggregation(
        &self,
        add: HashMap<String, EndpointSlice>,
        update: HashMap<String, EndpointSlice>,
        remove: &HashSet<String>,
    ) {
        // 1. Find affected services
        let mut affected_services: HashSet<String> = HashSet::new();

        {
            let ep_slices = self.ep_slices.read().unwrap();

            // Services affected by remove
            for ep_key in remove {
                if let Some((_, service_key)) = ep_slices.get(ep_key) {
                    affected_services.insert(service_key.clone());
                }
            }

            // Services affected by update
            for ep_key in update.keys() {
                if let Some((_, service_key)) = ep_slices.get(ep_key) {
                    affected_services.insert(service_key.clone());
                }
            }
        }

        // Services affected by add
        for ep_slice in add.values().chain(update.values()) {
            let service_key = extract_service_key(ep_slice);
            affected_services.insert(service_key);
        }

        // 2. Update ep_slices and service_to_slices
        let mut ep_slices = self.ep_slices.write().unwrap();
        let mut service_to_slices = self.service_to_slices.write().unwrap();

        // Process remove
        for ep_key in remove {
            if let Some((_, service_key)) = ep_slices.remove(ep_key) {
                if let Some(slice_list) = service_to_slices.get_mut(&service_key) {
                    slice_list.retain(|k| k != ep_key);
                    if slice_list.is_empty() {
                        service_to_slices.remove(&service_key);
                    }
                }
            }
        }

        // Process add and update
        for (ep_key, ep_slice) in add.iter().chain(update.iter()) {
            let service_key = extract_service_key(ep_slice);
            ep_slices.insert(ep_key.clone(), (ep_slice.clone(), service_key.clone()));

            let slice_list = service_to_slices.entry(service_key).or_default();
            if !slice_list.contains(ep_key) {
                slice_list.push(ep_key.clone());
            }
        }

        // 3. Re-aggregate affected services
        let current_service_lbs = self.service_lbs.load();
        let mut new_service_lbs = (**current_service_lbs).clone();

        for service_key in affected_services {
            if let Some(ep_keys) = service_to_slices.get(&service_key) {
                if ep_keys.is_empty() {
                    // Service has no EndpointSlices, remove it
                    new_service_lbs.remove(&service_key);
                } else {
                    // Collect all EndpointSlices for this service
                    let slices: Vec<EndpointSlice> = ep_keys
                        .iter()
                        .filter_map(|ep_key| ep_slices.get(ep_key).map(|(ep, _)| ep.clone()))
                        .collect();

                    // Re-create LoadBalancer with all slices
                    let lb = EndpointSliceLoadBalancer::new_with_slices(slices);
                    new_service_lbs.insert(service_key, lb);
                }
            } else {
                // Service not in service_to_slices, remove from LBs
                new_service_lbs.remove(&service_key);
            }
        }

        // 4. Atomically update service_lbs
        self.service_lbs.store(Arc::new(new_service_lbs));
    }

    /// Legacy method for compatibility - deprecated
    /// Use replace_all() or update_with_service_aggregation() instead
    #[deprecated(note = "Use replace_all() or update_with_service_aggregation() instead")]
    pub fn update(&self, add_or_update: HashMap<String, Arc<EndpointSliceLoadBalancer<S>>>, remove: &HashSet<String>) {
        let current = self.service_lbs.load();
        let mut new_map = (**current).clone();

        for key in remove {
            new_map.remove(key);
        }
        for (key, lb) in add_or_update {
            new_map.insert(key, lb);
        }

        self.service_lbs.store(Arc::new(new_map));
    }

    /// Legacy method for compatibility - deprecated
    /// Use replace_all() or update_with_service_aggregation() instead
    #[deprecated(note = "Use replace_all() or update_with_service_aggregation() instead")]
    pub fn apply_modifications<F>(&self, modify: F)
    where
        F: FnOnce(&mut HashMap<String, Arc<EndpointSliceLoadBalancer<S>>>),
    {
        let current = self.service_lbs.load();
        let mut new_map = (**current).clone();
        modify(&mut new_map);
        self.service_lbs.store(Arc::new(new_map));
    }
}

/// Extract service key from EndpointSlice label
/// Returns namespace/service-name format
fn extract_service_key(ep_slice: &EndpointSlice) -> String {
    let service_name = ep_slice
        .metadata
        .labels
        .as_ref()
        .and_then(|labels| labels.get("kubernetes.io/service-name"))
        .map(|s| s.as_str())
        .unwrap_or_else(|| {
            tracing::warn!(
                endpointslice = %ep_slice.metadata.name.as_deref().unwrap_or(""),
                "EndpointSlice missing kubernetes.io/service-name label, using name as fallback"
            );
            ep_slice.metadata.name.as_deref().unwrap_or("")
        });

    if let Some(namespace) = &ep_slice.metadata.namespace {
        format!("{}/{}", namespace, service_name)
    } else {
        service_name.to_string()
    }
}
