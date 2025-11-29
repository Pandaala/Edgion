use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use arc_swap::ArcSwap;
use once_cell::sync::Lazy;
use k8s_openapi::api::discovery::v1::EndpointSlice;

static GLOBAL_EP_SLICE_STORE: Lazy<Arc<EpSliceStore>> =
    Lazy::new(|| Arc::new(EpSliceStore::new()));

pub fn get_global_ep_slice_store() -> Arc<EpSliceStore> {
    GLOBAL_EP_SLICE_STORE.clone()
}

/// Type alias for the endpoint slice map
type EpSliceMap = HashMap<String, EndpointSlice>;

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

    /// Get an endpoint slice by key
    pub fn get(&self, key: &str) -> Option<EndpointSlice> {
        let map = self.ep_slices.load();
        map.get(key).cloned()
    }

    /// Get endpoint slice by service key (namespace/service-name)
    /// This searches for endpoint slices that belong to the given service
    pub fn get_by_service(&self, service_key: &str) -> Option<EndpointSlice> {
        let map = self.ep_slices.load();
        // EndpointSlice key format: "namespace/ep-slice-name"
        // Service key format: "namespace/service-name"
        // We need to find ep_slice with matching service label
        for (_, ep_slice) in map.iter() {
            if let Some(svc_key) = get_service_key(ep_slice) {
                if svc_key == service_key {
                    return Some(ep_slice.clone());
                }
            }
        }
        None
    }

    /// Execute a function with the endpoint slice reference
    pub fn with_ep_slice<F, R>(&self, key: &str, f: F) -> Option<R>
    where
        F: FnOnce(&EndpointSlice) -> R,
    {
        let map = self.ep_slices.load();
        map.get(key).map(f)
    }

    /// Replace all endpoint slices atomically
    pub fn replace_all(&self, ep_slices: HashMap<String, EndpointSlice>) {
        self.ep_slices.store(Arc::new(Arc::new(ep_slices)));
    }

    /// Update endpoint slices atomically (clone map + modify + swap)
    pub fn update(&self, add_or_update: HashMap<String, EndpointSlice>, remove: &HashSet<String>) {
        let current = self.ep_slices.load();
        let current_map: &EpSliceMap = &**current;
        let mut new_map: EpSliceMap = current_map.clone();
        
        for key in remove {
            new_map.remove(key);
        }
        for (key, ep_slice) in add_or_update {
            new_map.insert(key, ep_slice);
        }
        
        self.ep_slices.store(Arc::new(Arc::new(new_map)));
    }
}

const SERVICE_NAME_LABEL: &str = "kubernetes.io/service-name";

/// Extract service key from EndpointSlice
/// Returns "namespace/service-name" based on the kubernetes.io/service-name label
pub fn get_service_key(ep_slice: &EndpointSlice) -> Option<String> {
    let metadata = &ep_slice.metadata;
    let namespace = metadata.namespace.as_deref()?;
    let labels = metadata.labels.as_ref()?;
    let service_name = labels.get(SERVICE_NAME_LABEL)?;
    Some(format!("{}/{}", namespace, service_name))
}

