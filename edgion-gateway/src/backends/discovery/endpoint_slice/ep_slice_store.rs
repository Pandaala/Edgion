use super::discovery_impl::EndpointSliceExt;
use crate::lb::EdgionBackend;
use edgion_resources::constants::labels::k8s::SERVICE_NAME;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use parking_lot::{Mutex, RwLock};
use pingora_load_balancing::selection::{BackendSelection, RoundRobin};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, LazyLock};

/// Store for RoundRobin LoadBalancers (primary data layer + LB)
static ROUNDROBIN_STORE: LazyLock<Arc<EpSliceStore<RoundRobin>>> = LazyLock::new(|| Arc::new(EpSliceStore::new()));

pub fn get_roundrobin_store() -> Arc<EpSliceStore<RoundRobin>> {
    ROUNDROBIN_STORE.clone()
}

/// Data-layer store for EndpointSlice resources.
///
/// Selection goes through `compiled_selector::ServiceSelectorCell`. The data
/// layer (`ep_slices` + `service_to_slices`) is the single source of truth;
/// `get_backends_for_service` reads from it directly.
pub struct EpSliceStore<S>
where
    S: BackendSelection + 'static,
    S::Iter: pingora_load_balancing::selection::BackendIter,
{
    /// Cold path: Service key -> EndpointSlice keys
    service_to_slices: RwLock<HashMap<String, HashSet<String>>>,

    /// Cold path: EndpointSlice storage
    /// Key: namespace/endpointslice-name
    /// Value: (EndpointSlice, service_key)
    ep_slices: RwLock<HashMap<String, (EndpointSlice, String)>>,

    /// Lock for serialising data-layer writes.
    creation_lock: Mutex<()>,

    _marker: std::marker::PhantomData<S>,
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
            service_to_slices: RwLock::new(HashMap::new()),
            ep_slices: RwLock::new(HashMap::new()),
            creation_lock: Mutex::new(()),
            _marker: std::marker::PhantomData,
        }
    }

    /// Get slices for a service from data layer.
    ///
    /// No `creation_lock` needed: writers hold both write guards simultaneously,
    /// so the two RwLock acquisitions here observe a consistent snapshot.
    pub fn get_slices_for_service(&self, service_key: &str) -> Option<Vec<EndpointSlice>> {
        let service_to_slices = self.service_to_slices.read();
        let ep_slices = self.ep_slices.read();

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

    /// Replace all data (full_set). Returns all service_keys in the new data.
    pub fn replace_data_only(&self, data: HashMap<String, EndpointSlice>) -> HashSet<String> {
        let _lock = self.creation_lock.lock();

        let mut new_ep_slices: HashMap<String, (EndpointSlice, String)> = HashMap::new();
        let mut new_service_to_slices: HashMap<String, HashSet<String>> = HashMap::new();
        let mut all_services: HashSet<String> = HashSet::new();

        for (ep_key, ep_slice) in data {
            let service_key = extract_service_key(&ep_slice);
            all_services.insert(service_key.clone());
            new_ep_slices.insert(ep_key.clone(), (ep_slice, service_key.clone()));
            new_service_to_slices.entry(service_key).or_default().insert(ep_key);
        }

        // Hold both write guards simultaneously so readers cannot observe a
        // half-applied snapshot.
        let mut service_to_slices_guard = self.service_to_slices.write();
        let mut ep_slices_guard = self.ep_slices.write();
        *service_to_slices_guard = new_service_to_slices;
        *ep_slices_guard = new_ep_slices;

        tracing::debug!(services = all_services.len(), "Replaced data layer (data only)");

        all_services
    }

    /// Incrementally update data layer (partial_update). Returns affected service_keys.
    pub fn update_data_only(
        &self,
        add: HashMap<String, EndpointSlice>,
        update: HashMap<String, EndpointSlice>,
        remove: &HashSet<String>,
    ) -> HashSet<String> {
        let _lock = self.creation_lock.lock();

        let mut affected_services: HashSet<String> = HashSet::new();
        let mut service_to_slices = self.service_to_slices.write();
        let mut ep_slices = self.ep_slices.write();

        for ep_key in remove {
            if let Some((_, service_key)) = ep_slices.remove(ep_key) {
                affected_services.insert(service_key.clone());
                if let Some(slice_set) = service_to_slices.get_mut(&service_key) {
                    slice_set.remove(ep_key);
                    if slice_set.is_empty() {
                        service_to_slices.remove(&service_key);
                    }
                }
            }
        }

        for (ep_key, ep_slice) in update.iter() {
            let new_service_key = extract_service_key(ep_slice);

            if let Some((_, old_service_key)) = ep_slices.get(ep_key) {
                if old_service_key != &new_service_key {
                    affected_services.insert(old_service_key.clone());
                    if let Some(slice_set) = service_to_slices.get_mut(old_service_key) {
                        slice_set.remove(ep_key);
                        if slice_set.is_empty() {
                            service_to_slices.remove(old_service_key);
                        }
                    }
                }
            }

            affected_services.insert(new_service_key.clone());
            ep_slices.insert(ep_key.clone(), (ep_slice.clone(), new_service_key.clone()));
            service_to_slices
                .entry(new_service_key)
                .or_default()
                .insert(ep_key.clone());
        }

        for (ep_key, ep_slice) in add.iter() {
            let service_key = extract_service_key(ep_slice);
            affected_services.insert(service_key.clone());
            ep_slices.insert(ep_key.clone(), (ep_slice.clone(), service_key.clone()));
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

    /// Get current backend list for a service from the data layer.
    pub fn get_backends_for_service(&self, service_key: &str) -> Vec<EdgionBackend> {
        let Some(slices) = self.get_slices_for_service(service_key) else {
            return Vec::new();
        };
        if slices.is_empty() {
            return Vec::new();
        }

        // Fail closed when no EndpointSlice declares a usable port. Returning no
        // backends surfaces a BackendEndpointSliceNotFoundByRoundRobinDefault error
        // (HTTP 500) in the access log rather than silently misdirecting to a
        // guessed port. No tracing: data-plane request path (Iron rule 3).
        let Some(port) = slices
            .iter()
            .find_map(|s| s.ports.as_ref()?.first()?.port.map(|p| p as u16))
        else {
            return Vec::new();
        };

        let mut backends = BTreeSet::new();
        for slice in slices {
            backends.extend(slice.build_backends(port));
        }
        backends.into_iter().map(EdgionBackend::new).collect()
    }

    /// Number of EndpointSlice raw entries.
    pub fn slice_count(&self) -> usize {
        self.ep_slices.read().len()
    }

    /// Number of (service → slice keys) reverse-index entries.
    pub fn service_index_count(&self) -> usize {
        self.service_to_slices.read().len()
    }

    /// All EndpointSlice keys in the data layer.
    pub fn keys(&self) -> Vec<String> {
        self.ep_slices.read().keys().cloned().collect()
    }
}

/// Extract service key from EndpointSlice label.
/// Returns `namespace/service-name`.
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
mod stats_tests {
    use super::*;
    use pingora_load_balancing::selection::RoundRobin;

    #[test]
    fn counts_are_zero_initially() {
        let store: EpSliceStore<RoundRobin> = EpSliceStore::new();
        assert_eq!(store.slice_count(), 0);
        assert_eq!(store.service_index_count(), 0);
        assert!(store.keys().is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions, EndpointPort};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::collections::BTreeMap;

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

        let mut data = HashMap::new();
        let ep1 = create_test_endpoint_slice("svc-a-slice1", "default", "svc-a", vec!["10.0.0.1"]);
        let ep2 = create_test_endpoint_slice("svc-a-slice2", "default", "svc-a", vec!["10.0.0.2"]);
        let ep3 = create_test_endpoint_slice("svc-b-slice1", "default", "svc-b", vec!["10.0.0.3"]);

        data.insert("default/svc-a-slice1".to_string(), ep1);
        data.insert("default/svc-a-slice2".to_string(), ep2);
        data.insert("default/svc-b-slice1".to_string(), ep3);

        let services = store.replace_data_only(data);

        assert_eq!(services.len(), 2);
        assert!(services.contains("default/svc-a"));
        assert!(services.contains("default/svc-b"));

        assert!(store.get_slices_for_service("default/svc-a").is_some());
        assert_eq!(store.get_slices_for_service("default/svc-a").unwrap().len(), 2);
        assert!(store.get_slices_for_service("default/svc-b").is_some());
        assert_eq!(store.get_slices_for_service("default/svc-b").unwrap().len(), 1);
    }

    #[test]
    fn test_update_data_only_add() {
        let store = EpSliceStore::<RoundRobin>::new();

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

        let mut data = HashMap::new();
        data.insert(
            "default/svc-a-slice1".to_string(),
            create_test_endpoint_slice("svc-a-slice1", "default", "svc-a", vec!["10.0.0.1"]),
        );
        store.replace_data_only(data);

        let remove = HashSet::from(["default/svc-a-slice1".to_string()]);
        let affected = store.update_data_only(HashMap::new(), HashMap::new(), &remove);

        assert_eq!(affected.len(), 1);
        assert!(affected.contains("default/svc-a"));
        assert!(store.get_slices_for_service("default/svc-a").is_none());
    }

    #[test]
    fn test_update_data_only_service_key_change() {
        let store = EpSliceStore::<RoundRobin>::new();

        let mut data = HashMap::new();
        data.insert(
            "default/slice1".to_string(),
            create_test_endpoint_slice("slice1", "default", "svc-a", vec!["10.0.0.1"]),
        );
        store.replace_data_only(data);

        let update = HashMap::from([(
            "default/slice1".to_string(),
            create_test_endpoint_slice("slice1", "default", "svc-b", vec!["10.0.0.1"]),
        )]);

        let affected = store.update_data_only(HashMap::new(), update, &HashSet::new());

        assert!(affected.contains("default/svc-a"));
        assert!(affected.contains("default/svc-b"));
        assert!(store.get_slices_for_service("default/svc-a").is_none());
        assert!(store.get_slices_for_service("default/svc-b").is_some());
    }

    /// Build a slice with a single ready endpoint and a configurable `ports`.
    fn ready_slice_with_ports(
        name: &str,
        namespace: &str,
        service_name: &str,
        addr: &str,
        ports: Option<Vec<EndpointPort>>,
    ) -> EndpointSlice {
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
            endpoints: vec![Endpoint {
                addresses: vec![addr.to_string()],
                conditions: Some(EndpointConditions {
                    ready: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ports,
        }
    }

    #[test]
    fn test_get_backends_for_service_no_port_returns_empty() {
        let store = EpSliceStore::<RoundRobin>::new();

        // A ready endpoint but no declared port: must fail closed (no backends)
        // rather than fall back to a guessed default port (backends-F013).
        let mut data = HashMap::new();
        data.insert(
            "default/svc-noport-slice1".to_string(),
            ready_slice_with_ports("svc-noport-slice1", "default", "svc-noport", "10.0.0.1", None),
        );
        store.replace_data_only(data);

        assert!(store.get_backends_for_service("default/svc-noport").is_empty());
    }

    #[test]
    fn test_get_backends_for_service_with_port_builds_backend() {
        let store = EpSliceStore::<RoundRobin>::new();

        // Same ready endpoint, but with a declared port: a backend is built.
        // Confirms the empty result above is caused by the missing port, not readiness.
        let mut data = HashMap::new();
        data.insert(
            "default/svc-port-slice1".to_string(),
            ready_slice_with_ports(
                "svc-port-slice1",
                "default",
                "svc-port",
                "10.0.0.1",
                Some(vec![EndpointPort {
                    port: Some(80),
                    ..Default::default()
                }]),
            ),
        );
        store.replace_data_only(data);

        assert_eq!(store.get_backends_for_service("default/svc-port").len(), 1);
    }

    #[test]
    fn test_extract_service_key() {
        let ep = create_test_endpoint_slice("slice1", "default", "my-service", vec!["10.0.0.1"]);
        assert_eq!(extract_service_key(&ep), "default/my-service");

        let mut ep_no_ns = ep.clone();
        ep_no_ns.metadata.namespace = None;
        assert_eq!(extract_service_key(&ep_no_ns), "my-service");
    }
}
