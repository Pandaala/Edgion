use super::discovery_impl::{EndpointSliceExt, EndpointSliceLoadBalancer};
use crate::types::constants::labels::k8s::SERVICE_NAME;
use arc_swap::ArcSwap;
use futures::FutureExt;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use pingora_load_balancing::selection::{BackendSelection, RoundRobin};
use pingora_load_balancing::Backend;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::LazyLock;
use std::sync::{Arc, Mutex, RwLock};

/// Store for RoundRobin LoadBalancers (primary data layer + LB)
static ROUNDROBIN_STORE: LazyLock<Arc<EpSliceStore<RoundRobin>>> = LazyLock::new(|| Arc::new(EpSliceStore::new()));

pub fn get_roundrobin_store() -> Arc<EpSliceStore<RoundRobin>> {
    ROUNDROBIN_STORE.clone()
}

/// Generic store for endpoint slice load balancers
///
/// This store aggregates multiple EndpointSlices per Service and maintains
/// the mapping from service_key to LoadBalancer.
///
/// Design: RoundRobin store maintains both the data layer (ep_slices + service_to_slices)
/// and the Pingora LoadBalancer<RoundRobin>. LeastConn/EWMA/ConsistentHash algorithms
/// use `get_backends_for_service()` to read the backend list at selection time.
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
    /// Value: HashSet<namespace/endpointslice-name>
    /// Note: Only used by RoundRobin store (shared data layer)
    service_to_slices: RwLock<HashMap<String, HashSet<String>>>,

    /// Cold path: EndpointSlice storage
    /// Key: namespace/endpointslice-name
    /// Value: (EndpointSlice, service_key)
    /// Note: Only used by RoundRobin store (shared data layer)
    ep_slices: RwLock<HashMap<String, (EndpointSlice, String)>>,

    /// Lock for DCL pattern - protects data layer reads and LB creation/updates
    /// All operations that read from data layer and write to LB layer must hold this lock
    creation_lock: Mutex<()>,
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
            creation_lock: Mutex::new(()),
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

    // ==================== DCL Pattern Methods ====================

    /// DCL (Double-Checked Locking) pattern: Get existing LB or create new one
    ///
    /// Hot path: First tries lock-free ArcSwap read
    /// Cold path: Acquires lock, double-checks, then creates LB from data layer
    ///
    /// For RoundRobin store: reads from its own data layer
    /// For other stores: should call get_or_create_from_roundrobin instead
    pub fn get_or_create(&self, service_key: &str) -> Option<Arc<EndpointSliceLoadBalancer<S>>> {
        // 1. Fast path: lock-free ArcSwap read
        if let Some(lb) = self.service_lbs.load().get(service_key).cloned() {
            return Some(lb);
        }

        // 2. Slow path: acquire creation_lock
        let _lock = self.creation_lock.lock().unwrap();

        // 3. Double-check: another thread might have created it
        if let Some(lb) = self.service_lbs.load().get(service_key).cloned() {
            return Some(lb);
        }

        // 4. Get slices from data layer
        let slices = self.get_slices_for_service_internal(service_key)?;
        if slices.is_empty() {
            return None;
        }

        // 5. Create aggregated LB
        let lb = EndpointSliceLoadBalancer::new_with_slices(slices);

        // 6. Update ArcSwap (clone-on-write)
        let mut new_map = (**self.service_lbs.load()).clone();
        new_map.insert(service_key.to_string(), lb.clone());
        self.service_lbs.store(Arc::new(new_map));

        tracing::debug!(
            service_key = %service_key,
            "Created LB via DCL pattern"
        );

        Some(lb)
    }

    /// Get slices for a service from data layer (public, acquires lock)
    /// Used by other algorithm stores to fetch data from RoundRobin store
    pub fn get_slices_for_service(&self, service_key: &str) -> Option<Vec<EndpointSlice>> {
        let _lock = self.creation_lock.lock().unwrap();
        self.get_slices_for_service_internal(service_key)
    }

    /// Internal: Get slices for a service (caller must hold creation_lock)
    fn get_slices_for_service_internal(&self, service_key: &str) -> Option<Vec<EndpointSlice>> {
        let service_to_slices = self.service_to_slices.read().unwrap();
        let ep_slices = self.ep_slices.read().unwrap();

        let slice_keys = service_to_slices.get(service_key)?;
        let slices: Vec<EndpointSlice> = slice_keys
            .iter()
            .filter_map(|key| ep_slices.get(key).map(|(ep, _)| ep.clone()))
            .collect();

        if slices.is_empty() {
            None
        } else {
            Some(slices)
        }
    }

    // ==================== Data Layer Methods ====================

    /// Replace all data in data layer (full_set), does NOT create LBs
    /// Must acquire creation_lock
    /// Returns all service_keys in the new data
    pub fn replace_data_only(&self, data: HashMap<String, EndpointSlice>) -> HashSet<String> {
        let _lock = self.creation_lock.lock().unwrap();

        let mut new_ep_slices: HashMap<String, (EndpointSlice, String)> = HashMap::new();
        let mut new_service_to_slices: HashMap<String, HashSet<String>> = HashMap::new();
        let mut all_services: HashSet<String> = HashSet::new();

        for (ep_key, ep_slice) in data {
            let service_key = extract_service_key(&ep_slice);
            all_services.insert(service_key.clone());

            new_ep_slices.insert(ep_key.clone(), (ep_slice, service_key.clone()));
            new_service_to_slices.entry(service_key).or_default().insert(ep_key);
        }

        *self.service_to_slices.write().unwrap() = new_service_to_slices;
        *self.ep_slices.write().unwrap() = new_ep_slices;

        tracing::debug!(services = all_services.len(), "Replaced data layer (data only)");

        all_services
    }

    /// Incrementally update data layer (partial_update), does NOT create LBs
    /// Must acquire creation_lock
    /// Returns affected service_keys
    pub fn update_data_only(
        &self,
        add: HashMap<String, EndpointSlice>,
        update: HashMap<String, EndpointSlice>,
        remove: &HashSet<String>,
    ) -> HashSet<String> {
        let _lock = self.creation_lock.lock().unwrap();

        let mut affected_services: HashSet<String> = HashSet::new();
        let mut ep_slices = self.ep_slices.write().unwrap();
        let mut service_to_slices = self.service_to_slices.write().unwrap();

        // Process remove
        for ep_key in remove {
            if let Some((_, service_key)) = ep_slices.remove(ep_key) {
                affected_services.insert(service_key.clone());
                if let Some(slice_set) = service_to_slices.get_mut(&service_key) {
                    slice_set.remove(ep_key); // O(1) with HashSet
                    if slice_set.is_empty() {
                        service_to_slices.remove(&service_key);
                    }
                }
            }
        }

        // Process update (service_key might change!)
        for (ep_key, ep_slice) in update.iter() {
            let new_service_key = extract_service_key(ep_slice);

            // Check if old service_key changed
            if let Some((_, old_service_key)) = ep_slices.get(ep_key) {
                if old_service_key != &new_service_key {
                    // service_key changed, clean up old mapping
                    affected_services.insert(old_service_key.clone());
                    if let Some(slice_set) = service_to_slices.get_mut(old_service_key) {
                        slice_set.remove(ep_key); // O(1) with HashSet
                        if slice_set.is_empty() {
                            service_to_slices.remove(old_service_key);
                        }
                    }
                }
            }

            // Update data
            affected_services.insert(new_service_key.clone());
            ep_slices.insert(ep_key.clone(), (ep_slice.clone(), new_service_key.clone()));

            // HashSet insert is O(1) and handles duplicates automatically
            service_to_slices
                .entry(new_service_key)
                .or_default()
                .insert(ep_key.clone());
        }

        // Process add (new entries don't have old mappings to clean)
        for (ep_key, ep_slice) in add.iter() {
            let service_key = extract_service_key(ep_slice);
            affected_services.insert(service_key.clone());

            ep_slices.insert(ep_key.clone(), (ep_slice.clone(), service_key.clone()));

            // HashSet insert is O(1) and handles duplicates automatically
            service_to_slices.entry(service_key).or_default().insert(ep_key.clone());
        }

        tracing::debug!(
            affected = affected_services.len(),
            add = add.len(),
            update = update.len(),
            remove = remove.len(),
            "Updated data layer (data only)"
        );

        affected_services
    }

    /// Update LB if it exists, using latest data from data layer (in-place update)
    /// Must acquire creation_lock
    /// If data layer has no data for this service, removes the LB
    pub fn update_lb_if_exists(&self, service_key: &str) {
        let _lock = self.creation_lock.lock().unwrap();

        // Check if LB exists and get a reference
        let current = self.service_lbs.load();
        let lb = match current.get(service_key) {
            Some(lb) => lb.clone(),
            None => return,
        };

        // Get latest slices from data layer
        match self.get_slices_for_service_internal(service_key) {
            Some(slices) if !slices.is_empty() => {
                // In-place update: update discovery data, then refresh LB
                lb.update_slices(slices);
                lb.update_load_balancer().now_or_never();
                tracing::debug!(
                    service_key = %service_key,
                    "Updated existing LB in-place"
                );
            }
            _ => {
                // Data layer has no data, remove the LB
                let mut new_map = (**current).clone();
                new_map.remove(service_key);
                self.service_lbs.store(Arc::new(new_map));
                tracing::debug!(
                    service_key = %service_key,
                    "Removed LB (no data in data layer)"
                );
            }
        }
    }

    /// Get all service_keys that have existing LBs
    pub fn get_existing_service_keys(&self) -> Vec<String> {
        self.service_lbs.load().keys().cloned().collect()
    }

    /// Get current backend addresses for a service from data layer.
    pub fn get_backends_for_service(&self, service_key: &str) -> Vec<Backend> {
        let Some(slices) = self.get_slices_for_service(service_key) else {
            return Vec::new();
        };
        if slices.is_empty() {
            return Vec::new();
        }

        let port = slices
            .iter()
            .find_map(|s| s.ports.as_ref()?.first()?.port.map(|p| p as u16))
            .unwrap_or(8080);

        let mut backends = BTreeSet::new();
        for slice in slices {
            backends.extend(slice.build_backends(port));
        }
        backends.into_iter().collect()
    }

    /// Update LB if it exists, using external data source (in-place update).
    ///
    /// # Arguments
    /// * `service_key` - The service key to update
    /// * `slices_provider` - Function that provides EndpointSlices for the service (from RoundRobin store)
    pub fn update_lb_if_exists_with_provider<F>(&self, service_key: &str, slices_provider: F)
    where
        F: FnOnce(&str) -> Option<Vec<EndpointSlice>>,
    {
        let _lock = self.creation_lock.lock().unwrap();

        // Check if LB exists and get a reference
        let current = self.service_lbs.load();
        let lb = match current.get(service_key) {
            Some(lb) => lb.clone(),
            None => return,
        };

        // Get latest slices from external provider (RoundRobin store)
        match slices_provider(service_key) {
            Some(slices) if !slices.is_empty() => {
                // In-place update: update discovery data, then refresh LB
                lb.update_slices(slices);
                lb.update_load_balancer().now_or_never();
                tracing::debug!(
                    service_key = %service_key,
                    "Updated existing LB in-place (from shared data layer)"
                );
            }
            _ => {
                // No data in RoundRobin store, remove the LB
                let mut new_map = (**current).clone();
                new_map.remove(service_key);
                self.service_lbs.store(Arc::new(new_map));
                tracing::debug!(
                    service_key = %service_key,
                    "Removed LB (no data in shared data layer)"
                );
            }
        }
    }

    /// DCL pattern: Get existing LB or create new one using external data source.
    ///
    /// # Arguments
    /// * `service_key` - The service key to look up
    /// * `slices_provider` - Function that provides EndpointSlices for the service
    pub fn get_or_create_with_provider<F>(
        &self,
        service_key: &str,
        slices_provider: F,
    ) -> Option<Arc<EndpointSliceLoadBalancer<S>>>
    where
        F: FnOnce(&str) -> Option<Vec<EndpointSlice>>,
    {
        // 1. Fast path: lock-free ArcSwap read
        if let Some(lb) = self.service_lbs.load().get(service_key).cloned() {
            return Some(lb);
        }

        // 2. Slow path: acquire creation_lock
        let _lock = self.creation_lock.lock().unwrap();

        // 3. Double-check
        if let Some(lb) = self.service_lbs.load().get(service_key).cloned() {
            return Some(lb);
        }

        // 4. Get slices from external provider
        let slices = slices_provider(service_key)?;
        if slices.is_empty() {
            return None;
        }

        // 5. Create aggregated LB
        let lb = EndpointSliceLoadBalancer::new_with_slices(slices);

        // 6. Update ArcSwap
        let mut new_map = (**self.service_lbs.load()).clone();
        new_map.insert(service_key.to_string(), lb.clone());
        self.service_lbs.store(Arc::new(new_map));

        tracing::debug!(
            service_key = %service_key,
            "Created LB via DCL pattern (from external provider)"
        );

        Some(lb)
    }

    // ==================== Legacy Methods (deprecated) ====================

    /// Replace all endpoint slices atomically
    /// This aggregates EndpointSlices by service_key
    ///
    /// DEPRECATED: Use replace_data_only() + update_lb_if_exists() instead
    /// This method creates LBs for ALL services, which is wasteful
    #[deprecated(note = "Use replace_data_only() + update_lb_if_exists() for lazy LB creation")]
    pub fn replace_all(&self, data: HashMap<String, EndpointSlice>) {
        // 1. Group EndpointSlices by service_key
        let mut service_groups: HashMap<String, Vec<EndpointSlice>> = HashMap::new();
        let mut new_ep_slices: HashMap<String, (EndpointSlice, String)> = HashMap::new();
        let mut new_service_to_slices: HashMap<String, HashSet<String>> = HashMap::new();

        for (ep_key, ep_slice) in data {
            let service_key = extract_service_key(&ep_slice);

            service_groups
                .entry(service_key.clone())
                .or_default()
                .push(ep_slice.clone());

            new_ep_slices.insert(ep_key.clone(), (ep_slice, service_key.clone()));
            new_service_to_slices.entry(service_key).or_default().insert(ep_key);
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
    ///
    /// DEPRECATED: Use update_data_only() + update_lb_if_exists() instead
    /// This method creates/updates LBs for ALL affected services, which is wasteful
    #[deprecated(note = "Use update_data_only() + update_lb_if_exists() for lazy LB creation")]
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
                if let Some(slice_set) = service_to_slices.get_mut(&service_key) {
                    slice_set.remove(ep_key); // O(1) with HashSet
                    if slice_set.is_empty() {
                        service_to_slices.remove(&service_key);
                    }
                }
            }
        }

        // Process add and update
        for (ep_key, ep_slice) in add.iter().chain(update.iter()) {
            let service_key = extract_service_key(ep_slice);
            ep_slices.insert(ep_key.clone(), (ep_slice.clone(), service_key.clone()));

            // HashSet insert is O(1) and handles duplicates automatically
            service_to_slices.entry(service_key).or_default().insert(ep_key.clone());
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
}

/// Extract service key from EndpointSlice label
/// Returns namespace/service-name format
pub fn extract_service_key(ep_slice: &EndpointSlice) -> String {
    let service_name = ep_slice
        .metadata
        .labels
        .as_ref()
        .and_then(|labels| labels.get(SERVICE_NAME))
        .map(|s| s.as_str())
        .unwrap_or_else(|| {
            tracing::warn!(
                endpointslice = %ep_slice.metadata.name.as_deref().unwrap_or(""),
                label = SERVICE_NAME,
                "EndpointSlice missing service-name label, using name as fallback"
            );
            ep_slice.metadata.name.as_deref().unwrap_or("")
        });

    if let Some(namespace) = &ep_slice.metadata.namespace {
        format!("{}/{}", namespace, service_name)
    } else {
        service_name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::discovery::v1::{Endpoint, EndpointPort};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::collections::BTreeMap;
    use std::thread;

    /// Helper to create a test EndpointSlice
    fn create_test_endpoint_slice(name: &str, namespace: &str, service_name: &str, addrs: Vec<&str>) -> EndpointSlice {
        let mut labels = BTreeMap::new();
        labels.insert(SERVICE_NAME.to_string(), service_name.to_string());

        EndpointSlice {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(namespace.to_string()),
                labels: Some(labels),
                ..Default::default()
            },
            address_type: "IPv4".to_string(),
            endpoints: addrs
                .into_iter()
                .map(|addr| Endpoint {
                    addresses: vec![addr.to_string()],
                    ..Default::default()
                })
                .collect(),
            ports: Some(vec![EndpointPort {
                port: Some(80),
                ..Default::default()
            }]),
        }
    }

    #[test]
    fn test_replace_data_only() {
        let store = EpSliceStore::<RoundRobin>::new();

        // Create test data
        let mut data = HashMap::new();
        let ep1 = create_test_endpoint_slice("svc-a-slice1", "default", "svc-a", vec!["10.0.0.1"]);
        let ep2 = create_test_endpoint_slice("svc-a-slice2", "default", "svc-a", vec!["10.0.0.2"]);
        let ep3 = create_test_endpoint_slice("svc-b-slice1", "default", "svc-b", vec!["10.0.0.3"]);

        data.insert("default/svc-a-slice1".to_string(), ep1);
        data.insert("default/svc-a-slice2".to_string(), ep2);
        data.insert("default/svc-b-slice1".to_string(), ep3);

        // Replace data
        let services = store.replace_data_only(data);

        // Verify services
        assert_eq!(services.len(), 2);
        assert!(services.contains("default/svc-a"));
        assert!(services.contains("default/svc-b"));

        // Verify data layer
        assert!(store.get_slices_for_service("default/svc-a").is_some());
        assert_eq!(store.get_slices_for_service("default/svc-a").unwrap().len(), 2);
        assert!(store.get_slices_for_service("default/svc-b").is_some());
        assert_eq!(store.get_slices_for_service("default/svc-b").unwrap().len(), 1);

        // Verify LB not created yet (lazy)
        assert!(!store.contains("default/svc-a"));
        assert!(!store.contains("default/svc-b"));
    }

    #[test]
    fn test_get_or_create_dcl() {
        let store = EpSliceStore::<RoundRobin>::new();

        // Create test data
        let mut data = HashMap::new();
        let ep1 = create_test_endpoint_slice("svc-a-slice1", "default", "svc-a", vec!["10.0.0.1"]);
        data.insert("default/svc-a-slice1".to_string(), ep1);
        store.replace_data_only(data);

        // LB not created yet
        assert!(!store.contains("default/svc-a"));

        // Get or create
        let lb = store.get_or_create("default/svc-a");
        assert!(lb.is_some());

        // LB now exists
        assert!(store.contains("default/svc-a"));

        // Second call should return same LB (fast path)
        let lb2 = store.get_or_create("default/svc-a");
        assert!(lb2.is_some());

        // Non-existent service
        let lb_none = store.get_or_create("default/svc-nonexistent");
        assert!(lb_none.is_none());
    }

    #[test]
    fn test_update_data_only_add() {
        let store = EpSliceStore::<RoundRobin>::new();

        // Initial empty
        let add = HashMap::from([(
            "default/svc-a-slice1".to_string(),
            create_test_endpoint_slice("svc-a-slice1", "default", "svc-a", vec!["10.0.0.1"]),
        )]);

        let affected = store.update_data_only(add, HashMap::new(), &HashSet::new());

        assert_eq!(affected.len(), 1);
        assert!(affected.contains("default/svc-a"));
        assert!(store.get_slices_for_service("default/svc-a").is_some());
    }

    #[test]
    fn test_update_data_only_remove() {
        let store = EpSliceStore::<RoundRobin>::new();

        // Setup initial data
        let mut data = HashMap::new();
        data.insert(
            "default/svc-a-slice1".to_string(),
            create_test_endpoint_slice("svc-a-slice1", "default", "svc-a", vec!["10.0.0.1"]),
        );
        store.replace_data_only(data);

        // Remove
        let remove = HashSet::from(["default/svc-a-slice1".to_string()]);
        let affected = store.update_data_only(HashMap::new(), HashMap::new(), &remove);

        assert_eq!(affected.len(), 1);
        assert!(affected.contains("default/svc-a"));
        assert!(store.get_slices_for_service("default/svc-a").is_none());
    }

    #[test]
    fn test_update_data_only_service_key_change() {
        let store = EpSliceStore::<RoundRobin>::new();

        // Setup initial data with svc-a
        let mut data = HashMap::new();
        data.insert(
            "default/slice1".to_string(),
            create_test_endpoint_slice("slice1", "default", "svc-a", vec!["10.0.0.1"]),
        );
        store.replace_data_only(data);

        // Update: change service_key from svc-a to svc-b
        let update = HashMap::from([(
            "default/slice1".to_string(),
            create_test_endpoint_slice("slice1", "default", "svc-b", vec!["10.0.0.1"]),
        )]);

        let affected = store.update_data_only(HashMap::new(), update, &HashSet::new());

        // Both old and new service should be affected
        assert!(affected.contains("default/svc-a"));
        assert!(affected.contains("default/svc-b"));

        // Old service should have no slices, new service should have the slice
        assert!(store.get_slices_for_service("default/svc-a").is_none());
        assert!(store.get_slices_for_service("default/svc-b").is_some());
    }

    #[test]
    fn test_update_lb_if_exists() {
        let store = EpSliceStore::<RoundRobin>::new();

        // Setup data
        let mut data = HashMap::new();
        data.insert(
            "default/svc-a-slice1".to_string(),
            create_test_endpoint_slice("svc-a-slice1", "default", "svc-a", vec!["10.0.0.1"]),
        );
        store.replace_data_only(data);

        // Create LB
        let _ = store.get_or_create("default/svc-a");
        assert!(store.contains("default/svc-a"));

        // Update data (add another slice)
        let add = HashMap::from([(
            "default/svc-a-slice2".to_string(),
            create_test_endpoint_slice("svc-a-slice2", "default", "svc-a", vec!["10.0.0.2"]),
        )]);
        store.update_data_only(add, HashMap::new(), &HashSet::new());

        // Update existing LB
        store.update_lb_if_exists("default/svc-a");

        // LB still exists
        assert!(store.contains("default/svc-a"));
    }

    #[test]
    fn test_update_lb_if_exists_removes_when_no_data() {
        let store = EpSliceStore::<RoundRobin>::new();

        // Setup data and create LB
        let mut data = HashMap::new();
        data.insert(
            "default/svc-a-slice1".to_string(),
            create_test_endpoint_slice("svc-a-slice1", "default", "svc-a", vec!["10.0.0.1"]),
        );
        store.replace_data_only(data);
        let _ = store.get_or_create("default/svc-a");
        assert!(store.contains("default/svc-a"));

        // Remove data
        let remove = HashSet::from(["default/svc-a-slice1".to_string()]);
        store.update_data_only(HashMap::new(), HashMap::new(), &remove);

        // Update LB should remove it
        store.update_lb_if_exists("default/svc-a");
        assert!(!store.contains("default/svc-a"));
    }

    #[test]
    fn test_dcl_concurrent_access() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let store = Arc::new(EpSliceStore::<RoundRobin>::new());

        // Setup data
        let mut data = HashMap::new();
        data.insert(
            "default/svc-a-slice1".to_string(),
            create_test_endpoint_slice("svc-a-slice1", "default", "svc-a", vec!["10.0.0.1"]),
        );
        store.replace_data_only(data);

        let creation_count = Arc::new(AtomicUsize::new(0));
        let mut handles = vec![];

        // Spawn multiple threads to call get_or_create concurrently
        for _ in 0..10 {
            let store_clone = store.clone();
            let count_clone = creation_count.clone();

            let handle = thread::spawn(move || {
                let lb = store_clone.get_or_create("default/svc-a");
                if lb.is_some() {
                    count_clone.fetch_add(1, Ordering::SeqCst);
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // All threads should have gotten a valid LB
        assert_eq!(creation_count.load(Ordering::SeqCst), 10);

        // Only one LB should exist
        assert!(store.contains("default/svc-a"));
        assert_eq!(store.get_existing_service_keys().len(), 1);
    }

    #[test]
    fn test_extract_service_key() {
        let ep = create_test_endpoint_slice("slice1", "default", "my-service", vec!["10.0.0.1"]);
        assert_eq!(extract_service_key(&ep), "default/my-service");

        // No namespace
        let mut ep_no_ns = ep.clone();
        ep_no_ns.metadata.namespace = None;
        assert_eq!(extract_service_key(&ep_no_ns), "my-service");
    }
}
