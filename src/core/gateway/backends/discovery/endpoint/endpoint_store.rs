use super::discovery_impl::{EndpointExt, EndpointLoadBalancer};
use arc_swap::ArcSwap;
use k8s_openapi::api::core::v1::Endpoints;
use pingora_load_balancing::selection::{BackendSelection, RoundRobin};
use pingora_load_balancing::Backend;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

/// Store for RoundRobin LoadBalancers (primary data layer + LB)
static ROUNDROBIN_STORE: LazyLock<Arc<EndpointStore<RoundRobin>>> = LazyLock::new(|| Arc::new(EndpointStore::new()));

pub fn get_endpoint_roundrobin_store() -> Arc<EndpointStore<RoundRobin>> {
    ROUNDROBIN_STORE.clone()
}

/// Generic store for endpoint load balancers
///
/// Design: RoundRobin store maintains both the data layer (ep_data) and the
/// Pingora LoadBalancer<RoundRobin>. LeastConn/EWMA/ConsistentHash algorithms
/// use `get_backends_for_service()` to read the backend list at selection time.
pub struct EndpointStore<S>
where
    S: BackendSelection + 'static,
    S::Iter: pingora_load_balancing::selection::BackendIter,
{
    /// Hot path: Service key -> LoadBalancer
    /// Key: namespace/service-name
    endpoints: ArcSwap<HashMap<String, Arc<EndpointLoadBalancer<S>>>>,

    /// Cold path: Endpoint data storage (only used by RoundRobin store)
    /// Key: namespace/service-name (same as endpoints key)
    /// Value: Endpoints resource
    ep_data: RwLock<HashMap<String, Endpoints>>,

    /// Lock for DCL pattern - protects data layer reads and LB creation/updates
    creation_lock: Mutex<()>,
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
            ep_data: RwLock::new(HashMap::new()),
            creation_lock: Mutex::new(()),
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

    // ==================== DCL Pattern Methods ====================

    /// DCL (Double-Checked Locking) pattern: Get existing LB or create new one
    ///
    /// Hot path: First tries lock-free ArcSwap read
    /// Cold path: Acquires lock, double-checks, then creates LB from data layer
    pub fn get_or_create(&self, service_key: &str) -> Option<Arc<EndpointLoadBalancer<S>>> {
        // 1. Fast path: lock-free ArcSwap read
        if let Some(lb) = self.endpoints.load().get(service_key).cloned() {
            return Some(lb);
        }

        // 2. Slow path: acquire creation_lock
        let _lock = self.creation_lock.lock().unwrap();

        // 3. Double-check
        if let Some(lb) = self.endpoints.load().get(service_key).cloned() {
            return Some(lb);
        }

        // 4. Get endpoint from data layer
        let ep_data = self.ep_data.read().unwrap();
        let endpoint = ep_data.get(service_key)?.clone();
        drop(ep_data);

        // 5. Create LB
        let lb = EndpointLoadBalancer::new(endpoint);

        // 6. Update ArcSwap
        let mut new_map = (**self.endpoints.load()).clone();
        new_map.insert(service_key.to_string(), lb.clone());
        self.endpoints.store(Arc::new(new_map));

        tracing::debug!(
            service_key = %service_key,
            "Created Endpoint LB via DCL pattern"
        );

        Some(lb)
    }

    /// Get endpoint for a service from data layer (public, acquires lock)
    /// Used by other algorithm stores to fetch data from RoundRobin store
    pub fn get_endpoint_for_service(&self, service_key: &str) -> Option<Endpoints> {
        let _lock = self.creation_lock.lock().unwrap();
        self.ep_data.read().unwrap().get(service_key).cloned()
    }

    /// DCL pattern: Get existing LB or create new one using external data source.
    pub fn get_or_create_with_provider<F>(
        &self,
        service_key: &str,
        endpoint_provider: F,
    ) -> Option<Arc<EndpointLoadBalancer<S>>>
    where
        F: FnOnce(&str) -> Option<Endpoints>,
    {
        // 1. Fast path
        if let Some(lb) = self.endpoints.load().get(service_key).cloned() {
            return Some(lb);
        }

        // 2. Slow path
        let _lock = self.creation_lock.lock().unwrap();

        // 3. Double-check
        if let Some(lb) = self.endpoints.load().get(service_key).cloned() {
            return Some(lb);
        }

        // 4. Get endpoint from external provider
        let endpoint = endpoint_provider(service_key)?;

        // 5. Create LB
        let lb = EndpointLoadBalancer::new(endpoint);

        // 6. Update ArcSwap
        let mut new_map = (**self.endpoints.load()).clone();
        new_map.insert(service_key.to_string(), lb.clone());
        self.endpoints.store(Arc::new(new_map));

        tracing::debug!(
            service_key = %service_key,
            "Created Endpoint LB via DCL pattern (from external provider)"
        );

        Some(lb)
    }

    // ==================== Data Layer Methods ====================

    /// Replace all data in data layer (full_set), does NOT create LBs
    /// Returns all service_keys in the new data
    pub fn replace_data_only(&self, data: HashMap<String, Endpoints>) -> HashSet<String> {
        let _lock = self.creation_lock.lock().unwrap();

        let keys: HashSet<String> = data.keys().cloned().collect();
        *self.ep_data.write().unwrap() = data;

        tracing::debug!(services = keys.len(), "Replaced Endpoint data layer (data only)");

        keys
    }

    /// Incrementally update data layer (partial_update), does NOT create LBs
    /// Returns affected service_keys
    pub fn update_data_only(
        &self,
        add: HashMap<String, Endpoints>,
        update: HashMap<String, Endpoints>,
        remove: &HashSet<String>,
    ) -> HashSet<String> {
        let _lock = self.creation_lock.lock().unwrap();

        let mut affected_services: HashSet<String> = HashSet::new();
        let mut ep_data = self.ep_data.write().unwrap();

        // Process remove
        for key in remove {
            if ep_data.remove(key).is_some() {
                affected_services.insert(key.clone());
            }
        }

        // Process add and update
        for (key, endpoint) in add.into_iter().chain(update.into_iter()) {
            affected_services.insert(key.clone());
            ep_data.insert(key, endpoint);
        }

        tracing::debug!(
            affected = affected_services.len(),
            "Updated Endpoint data layer (data only)"
        );

        affected_services
    }

    /// Update LB if it exists, using latest data from data layer
    /// If data layer has no data for this service, removes the LB
    pub fn update_lb_if_exists(&self, service_key: &str) {
        let _lock = self.creation_lock.lock().unwrap();

        // Check if LB exists
        let current = self.endpoints.load();
        let lb = match current.get(service_key) {
            Some(lb) => lb.clone(),
            None => return,
        };

        // Get latest endpoint from data layer
        let ep_data = self.ep_data.read().unwrap();
        let endpoint = match ep_data.get(service_key) {
            Some(ep) => ep.clone(),
            None => {
                // Data layer has no data, remove the LB
                drop(ep_data);
                let mut new_map = (**current).clone();
                new_map.remove(service_key);
                self.endpoints.store(Arc::new(new_map));
                tracing::debug!(
                    service_key = %service_key,
                    "Removed Endpoint LB (no data in data layer)"
                );
                return;
            }
        };
        drop(ep_data);

        // Update LB in-place
        if let Err(e) = lb.update(endpoint) {
            tracing::error!(key = %service_key, error = %e, "Failed to update Endpoint LB data");
            return;
        }

        // Refresh LoadBalancer
        use futures::FutureExt;
        if let Some(Err(e)) = lb.update_load_balancer().now_or_never() {
            tracing::warn!(key = %service_key, error = %e, "Failed to refresh Endpoint LB");
        } else {
            tracing::debug!(
                service_key = %service_key,
                "Updated existing Endpoint LB"
            );
        }
    }

    /// Get all service_keys that have existing LBs
    pub fn get_existing_service_keys(&self) -> Vec<String> {
        self.endpoints.load().keys().cloned().collect()
    }

    /// Get current backend addresses for a service from data layer.
    pub fn get_backends_for_service(&self, service_key: &str) -> Vec<Backend> {
        let Some(endpoint) = self.get_endpoint_for_service(service_key) else {
            return Vec::new();
        };

        let port = endpoint
            .subsets
            .as_ref()
            .and_then(|subsets| subsets.first())
            .and_then(|subset| subset.ports.as_ref())
            .and_then(|ports| ports.first())
            .map(|p| p.port as u16)
            .unwrap_or(80);

        endpoint.build_backends(port).into_iter().collect()
    }

    /// Update LB if it exists, using external data source.
    ///
    /// # Arguments
    /// * `service_key` - The service key to update
    /// * `endpoint_provider` - Function that provides Endpoints for the service (from RoundRobin store)
    pub fn update_lb_if_exists_with_provider<F>(&self, service_key: &str, endpoint_provider: F)
    where
        F: FnOnce(&str) -> Option<Endpoints>,
    {
        let _lock = self.creation_lock.lock().unwrap();

        // Check if LB exists
        let current = self.endpoints.load();
        let lb = match current.get(service_key) {
            Some(lb) => lb.clone(),
            None => return,
        };

        // Get latest endpoint from external provider (RoundRobin store)
        let endpoint = match endpoint_provider(service_key) {
            Some(ep) => ep,
            None => {
                // No data in RoundRobin store, remove the LB
                let mut new_map = (**current).clone();
                new_map.remove(service_key);
                self.endpoints.store(Arc::new(new_map));
                tracing::debug!(
                    service_key = %service_key,
                    "Removed Endpoint LB (no data in shared data layer)"
                );
                return;
            }
        };

        // Update LB in-place
        if let Err(e) = lb.update(endpoint) {
            tracing::error!(key = %service_key, error = %e, "Failed to update Endpoint LB data");
            return;
        }

        // Refresh LoadBalancer
        use futures::FutureExt;
        if let Some(Err(e)) = lb.update_load_balancer().now_or_never() {
            tracing::warn!(key = %service_key, error = %e, "Failed to refresh Endpoint LB");
        } else {
            tracing::debug!(
                service_key = %service_key,
                "Updated existing Endpoint LB (from shared data layer)"
            );
        }
    }

    // ==================== Legacy Methods (deprecated) ====================

    /// Replace all endpoints atomically
    ///
    /// DEPRECATED: Use replace_data_only() + update_lb_if_exists() instead
    #[deprecated(note = "Use replace_data_only() + update_lb_if_exists() for lazy LB creation")]
    pub fn replace_all(&self, endpoints: HashMap<String, Arc<EndpointLoadBalancer<S>>>) {
        self.endpoints.store(Arc::new(endpoints));
    }

    /// Update endpoints atomically (clone map + modify + swap)
    ///
    /// DEPRECATED: Use update_data_only() + update_lb_if_exists() instead
    #[deprecated(note = "Use update_data_only() + update_lb_if_exists() for lazy LB creation")]
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

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::core::v1::{EndpointAddress, EndpointPort, EndpointSubset};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::thread;

    /// Helper to create a test Endpoints resource
    fn create_test_endpoints(name: &str, namespace: &str, addrs: Vec<&str>) -> Endpoints {
        Endpoints {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            subsets: Some(vec![EndpointSubset {
                addresses: Some(
                    addrs
                        .into_iter()
                        .map(|ip| EndpointAddress {
                            ip: ip.to_string(),
                            ..Default::default()
                        })
                        .collect(),
                ),
                ports: Some(vec![EndpointPort {
                    port: 80,
                    ..Default::default()
                }]),
                ..Default::default()
            }]),
        }
    }

    #[test]
    fn test_replace_data_only() {
        let store = EndpointStore::<RoundRobin>::new();

        let mut data = HashMap::new();
        data.insert(
            "default/svc-a".to_string(),
            create_test_endpoints("svc-a", "default", vec!["10.0.0.1"]),
        );
        data.insert(
            "default/svc-b".to_string(),
            create_test_endpoints("svc-b", "default", vec!["10.0.0.2"]),
        );

        let services = store.replace_data_only(data);

        assert_eq!(services.len(), 2);
        assert!(services.contains("default/svc-a"));
        assert!(services.contains("default/svc-b"));

        // Verify data layer
        assert!(store.get_endpoint_for_service("default/svc-a").is_some());
        assert!(store.get_endpoint_for_service("default/svc-b").is_some());

        // Verify LB not created yet (lazy)
        assert!(!store.contains("default/svc-a"));
        assert!(!store.contains("default/svc-b"));
    }

    #[test]
    fn test_get_or_create_dcl() {
        let store = EndpointStore::<RoundRobin>::new();

        let mut data = HashMap::new();
        data.insert(
            "default/svc-a".to_string(),
            create_test_endpoints("svc-a", "default", vec!["10.0.0.1"]),
        );
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
        let store = EndpointStore::<RoundRobin>::new();

        let add = HashMap::from([(
            "default/svc-a".to_string(),
            create_test_endpoints("svc-a", "default", vec!["10.0.0.1"]),
        )]);

        let affected = store.update_data_only(add, HashMap::new(), &HashSet::new());

        assert_eq!(affected.len(), 1);
        assert!(affected.contains("default/svc-a"));
        assert!(store.get_endpoint_for_service("default/svc-a").is_some());
    }

    #[test]
    fn test_update_data_only_remove() {
        let store = EndpointStore::<RoundRobin>::new();

        // Setup initial data
        let mut data = HashMap::new();
        data.insert(
            "default/svc-a".to_string(),
            create_test_endpoints("svc-a", "default", vec!["10.0.0.1"]),
        );
        store.replace_data_only(data);

        // Remove
        let remove = HashSet::from(["default/svc-a".to_string()]);
        let affected = store.update_data_only(HashMap::new(), HashMap::new(), &remove);

        assert_eq!(affected.len(), 1);
        assert!(affected.contains("default/svc-a"));
        assert!(store.get_endpoint_for_service("default/svc-a").is_none());
    }

    #[test]
    fn test_update_lb_if_exists_removes_when_no_data() {
        let store = EndpointStore::<RoundRobin>::new();

        // Setup data and create LB
        let mut data = HashMap::new();
        data.insert(
            "default/svc-a".to_string(),
            create_test_endpoints("svc-a", "default", vec!["10.0.0.1"]),
        );
        store.replace_data_only(data);
        let _ = store.get_or_create("default/svc-a");
        assert!(store.contains("default/svc-a"));

        // Remove data
        let remove = HashSet::from(["default/svc-a".to_string()]);
        store.update_data_only(HashMap::new(), HashMap::new(), &remove);

        // Update LB should remove it
        store.update_lb_if_exists("default/svc-a");
        assert!(!store.contains("default/svc-a"));
    }

    #[test]
    fn test_dcl_concurrent_access() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let store = Arc::new(EndpointStore::<RoundRobin>::new());

        // Setup data
        let mut data = HashMap::new();
        data.insert(
            "default/svc-a".to_string(),
            create_test_endpoints("svc-a", "default", vec!["10.0.0.1"]),
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
}
