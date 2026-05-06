use super::discovery_impl::EndpointExt;
use crate::core::gateway::lb::EdgionBackend;
use k8s_openapi::api::core::v1::Endpoints;
use parking_lot::{Mutex, RwLock};
use pingora_load_balancing::selection::{BackendSelection, RoundRobin};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

/// Store for RoundRobin LoadBalancers (primary data layer + LB)
static ROUNDROBIN_STORE: LazyLock<Arc<EndpointStore<RoundRobin>>> = LazyLock::new(|| Arc::new(EndpointStore::new()));

pub fn get_endpoint_roundrobin_store() -> Arc<EndpointStore<RoundRobin>> {
    ROUNDROBIN_STORE.clone()
}

/// Data-layer store for Endpoints resources.
///
/// The LB cache (`endpoints` ArcSwap) has been removed in Stage 5. Selection
/// now goes through `compiled_selector::ServiceSelectorCell`, which is hydrated
/// lazily from `get_backends_for_service` on the first request. The data layer
/// (`ep_data`) is the single source of truth; `get_backends_for_service` reads
/// from it directly.
pub struct EndpointStore<S>
where
    S: BackendSelection + 'static,
    S::Iter: pingora_load_balancing::selection::BackendIter,
{
    /// Endpoint data storage.
    /// Key: namespace/service-name
    ep_data: RwLock<HashMap<String, Endpoints>>,

    /// Lock used by replace_data_only / update_data_only to serialise writes.
    creation_lock: Mutex<()>,

    _marker: std::marker::PhantomData<S>,
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
            ep_data: RwLock::new(HashMap::new()),
            creation_lock: Mutex::new(()),
            _marker: std::marker::PhantomData,
        }
    }

    /// Get endpoint for a service from data layer.
    pub fn get_endpoint_for_service(&self, service_key: &str) -> Option<Endpoints> {
        self.ep_data.read().get(service_key).cloned()
    }

    // ==================== Data Layer Methods ====================

    /// Replace all data in data layer (full_set). Returns all service_keys in new data.
    pub fn replace_data_only(&self, data: HashMap<String, Endpoints>) -> HashSet<String> {
        let _lock = self.creation_lock.lock();

        let keys: HashSet<String> = data.keys().cloned().collect();
        *self.ep_data.write() = data;

        tracing::debug!(services = keys.len(), "Replaced Endpoint data layer (data only)");

        keys
    }

    /// Incrementally update data layer (partial_update). Returns affected service_keys.
    pub fn update_data_only(
        &self,
        add: HashMap<String, Endpoints>,
        update: HashMap<String, Endpoints>,
        remove: &HashSet<String>,
    ) -> HashSet<String> {
        let _lock = self.creation_lock.lock();

        let mut affected_services: HashSet<String> = HashSet::new();
        let mut ep_data = self.ep_data.write();

        for key in remove {
            if ep_data.remove(key).is_some() {
                affected_services.insert(key.clone());
            }
        }

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

    /// Get current backend list for a service from the data layer.
    pub fn get_backends_for_service(&self, service_key: &str) -> Vec<EdgionBackend> {
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

        endpoint
            .build_backends(port)
            .into_iter()
            .map(EdgionBackend::new)
            .collect()
    }

    /// Number of raw Endpoints data entries.
    pub fn ep_data_count(&self) -> usize {
        self.ep_data.read().len()
    }

    /// All service keys in the data layer.
    pub fn keys(&self) -> Vec<String> {
        self.ep_data.read().keys().cloned().collect()
    }
}

#[cfg(test)]
mod stats_tests {
    use super::*;
    use pingora_load_balancing::selection::RoundRobin;

    fn empty_endpoints(name: &str) -> Endpoints {
        Endpoints {
            metadata: kube::api::ObjectMeta {
                name: Some(name.to_string()),
                ..Default::default()
            },
            subsets: None,
        }
    }

    #[test]
    fn counts_are_zero_initially() {
        let store: EndpointStore<RoundRobin> = EndpointStore::new();
        assert_eq!(store.ep_data_count(), 0);
        assert!(store.keys().is_empty());
    }

    #[test]
    fn ep_data_count_tracks_writes() {
        let store: EndpointStore<RoundRobin> = EndpointStore::new();
        let mut data = HashMap::new();
        data.insert("default/svc-a".to_string(), empty_endpoints("svc-a"));
        store.replace_data_only(data);
        assert_eq!(store.ep_data_count(), 1);
        assert_eq!(store.keys(), vec!["default/svc-a".to_string()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::core::v1::{EndpointAddress, EndpointPort, EndpointSubset};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

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

        assert!(store.get_endpoint_for_service("default/svc-a").is_some());
        assert!(store.get_endpoint_for_service("default/svc-b").is_some());
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

        let mut data = HashMap::new();
        data.insert(
            "default/svc-a".to_string(),
            create_test_endpoints("svc-a", "default", vec!["10.0.0.1"]),
        );
        store.replace_data_only(data);

        let remove = HashSet::from(["default/svc-a".to_string()]);
        let affected = store.update_data_only(HashMap::new(), HashMap::new(), &remove);

        assert_eq!(affected.len(), 1);
        assert!(affected.contains("default/svc-a"));
        assert!(store.get_endpoint_for_service("default/svc-a").is_none());
    }

    #[test]
    fn test_get_backends_for_service() {
        let store = EndpointStore::<RoundRobin>::new();

        let mut data = HashMap::new();
        data.insert(
            "default/svc-a".to_string(),
            create_test_endpoints("svc-a", "default", vec!["10.0.0.1", "10.0.0.2"]),
        );
        store.replace_data_only(data);

        let backends = store.get_backends_for_service("default/svc-a");
        assert_eq!(backends.len(), 2);

        let backends_none = store.get_backends_for_service("default/svc-nonexistent");
        assert!(backends_none.is_empty());
    }
}
