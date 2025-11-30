use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use arc_swap::ArcSwap;
use once_cell::sync::Lazy;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use super::discovery_impl::EndpointSliceDiscovery;

static GLOBAL_EP_SLICE_STORE: Lazy<Arc<EpSliceStore>> =
    Lazy::new(|| Arc::new(EpSliceStore::new()));

pub fn get_global_ep_slice_store() -> Arc<EpSliceStore> {
    GLOBAL_EP_SLICE_STORE.clone()
}

/// Type alias for the endpoint slice discovery map
type EpSliceMap = HashMap<String, Arc<EndpointSliceDiscovery>>;

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

    /// Get an endpoint slice discovery by key
    pub fn get(&self, key: &str) -> Option<Arc<EndpointSliceDiscovery>> {
        let map = self.ep_slices.load();
        map.get(key).cloned()
    }

    /// Get endpoint slice discovery by service key (namespace/service-name)
    /// This searches for endpoint slices that belong to the given service
    pub fn get_by_service(&self, service_key: &str) -> Option<Arc<EndpointSliceDiscovery>> {
        let map = self.ep_slices.load();
        // EndpointSlice key format: "namespace/ep-slice-name"
        // Service key format: "namespace/service-name"
        // We need to find ep_slice with matching service label
        for (_, ep_slice_discovery) in map.iter() {
            if let Some(svc_key) = ep_slice_discovery.service_key() {
                if svc_key == service_key {
                    return Some(Arc::clone(ep_slice_discovery));
                }
            }
        }
        None
    }

    /// Execute a function with the endpoint slice discovery reference
    pub fn with_ep_slice<F, R>(&self, key: &str, f: F) -> Option<R>
    where
        F: FnOnce(&Arc<EndpointSliceDiscovery>) -> R,
    {
        let map = self.ep_slices.load();
        map.get(key).map(f)
    }

    /// Replace all endpoint slices atomically
    pub fn replace_all(&self, ep_slices: HashMap<String, Arc<EndpointSliceDiscovery>>) {
        self.ep_slices.store(Arc::new(Arc::new(ep_slices)));
    }

    /// Update endpoint slices atomically (clone map + modify + swap)
    pub fn update(&self, add_or_update: HashMap<String, Arc<EndpointSliceDiscovery>>, remove: &HashSet<String>) {
        let current = self.ep_slices.load();
        let current_map: &EpSliceMap = &**current;
        let mut new_map: EpSliceMap = current_map.clone();
        
        for key in remove {
            new_map.remove(key);
        }
        for (key, ep_slice_discovery) in add_or_update {
            new_map.insert(key, ep_slice_discovery);
        }
        
        self.ep_slices.store(Arc::new(Arc::new(new_map)));
    }
    
    /// Update an existing EndpointSlice in-place without rebuilding the map
    /// Returns Ok(true) if updated, Ok(false) if key not found, Err on update failure
    pub fn update_in_place(&self, key: &str, new_endpoint_slice: k8s_openapi::api::discovery::v1::EndpointSlice) -> Result<bool, String> {
        let map = self.ep_slices.load();
        if let Some(discovery) = map.get(key) {
            discovery.update(new_endpoint_slice)?;
            tracing::debug!(key = %key, "Updated EndpointSlice in-place");
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

/// Extract service key from EndpointSlice (for backward compatibility)
/// Returns "namespace/service-name" based on the kubernetes.io/service-name label
pub fn get_service_key(ep_slice: &EndpointSlice) -> Option<String> {
    const SERVICE_NAME_LABEL: &str = "kubernetes.io/service-name";
    let metadata = &ep_slice.metadata;
    let namespace = metadata.namespace.as_deref()?;
    let labels = metadata.labels.as_ref()?;
    let service_name = labels.get(SERVICE_NAME_LABEL)?;
    Some(format!("{}/{}", namespace, service_name))
}

