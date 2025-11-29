use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use arc_swap::ArcSwap;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use k8s_openapi::api::core::v1::Service;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use pingora_core::protocols::l4::socket::SocketAddr;
use crate::types::{HTTPBackendRef, MatchInfo};

static GLOBAL_SERVICE_MGR: Lazy<Arc<ServiceMgr>> =
    Lazy::new(|| Arc::new(ServiceMgr::new()));

pub fn get_global_service_mgr() -> Arc<ServiceMgr> {
    GLOBAL_SERVICE_MGR.clone()
}

/// Upstream service data with internal mutability
pub struct UpstreamService {
    pub service: RwLock<Option<Service>>,
    pub ep_slice: RwLock<Option<EndpointSlice>>,
}

impl UpstreamService {
    pub fn new() -> Self {
        Self {
            service: RwLock::new(None),
            ep_slice: RwLock::new(None),
        }
    }

    pub fn with_service(service: Service) -> Self {
        Self {
            service: RwLock::new(Some(service)),
            ep_slice: RwLock::new(None),
        }
    }
}

/// Type alias for the service map
type ServiceMap = HashMap<String, UpstreamService>;

pub struct ServiceMgr {
    services: ArcSwap<Arc<ServiceMap>>,
}

impl ServiceMgr {
    pub fn new() -> ServiceMgr {
        Self {
            services: ArcSwap::from_pointee(Arc::new(HashMap::new())),
        }
    }

    pub fn get_peer(&self, match_info: &MatchInfo, br: &HTTPBackendRef) -> Option<SocketAddr> {
        
        None
    }

    /// Check if a service exists
    pub fn contains(&self, key: &str) -> bool {
        let map = self.services.load();
        map.contains_key(key)
    }

    /// Check if a service has ep_slice
    pub fn has_ep_slice(&self, key: &str) -> bool {
        let map = self.services.load();
        map.get(key).map(|u| u.ep_slice.read().is_some()).unwrap_or(false)
    }

    /// Execute a function with the upstream service reference
    pub fn with_upstream<F, R>(&self, key: &str, f: F) -> Option<R>
    where
        F: FnOnce(&UpstreamService) -> R,
    {
        let map = self.services.load();
        map.get(key).map(f)
    }

    /// Replace all services atomically
    pub fn replace_all(&self, services: HashMap<String, UpstreamService>) {
        self.services.store(Arc::new(Arc::new(services)));
    }

    /// Update services atomically (clone map + modify + swap)
    pub fn update(&self, add_or_update: HashMap<String, UpstreamService>, remove: &HashSet<String>) {
        let current = self.services.load();
        let current_map: &ServiceMap = &**current;
        let mut new_map: ServiceMap = clone_service_map(current_map);
        
        for key in remove {
            new_map.remove(key);
        }
        for (key, service) in add_or_update {
            new_map.insert(key, service);
        }
        
        self.services.store(Arc::new(Arc::new(new_map)));
    }

    /// Update EndpointSlice for a service (in-place update, no map clone)
    pub fn set_ep_slice(&self, service_key: &str, ep_slice: EndpointSlice) {
        let current = self.services.load();
        if let Some(upstream) = current.get(service_key) {
            // In-place update using RwLock
            *upstream.ep_slice.write() = Some(ep_slice);
        } else {
            // Need to add new entry - clone map
            let current_map: &ServiceMap = &**current;
            let mut new_map = clone_service_map(current_map);
            new_map.insert(service_key.to_string(), UpstreamService {
                service: RwLock::new(None),
                ep_slice: RwLock::new(Some(ep_slice)),
            });
            self.services.store(Arc::new(Arc::new(new_map)));
        }
    }

    /// Remove EndpointSlice for a service (in-place update, no map clone)
    pub fn remove_ep_slice(&self, service_key: &str) {
        let current = self.services.load();
        if let Some(upstream) = current.get(service_key) {
            *upstream.ep_slice.write() = None;
        }
    }
}

/// Clone a service map (since UpstreamService contains RwLock, we need manual clone)
fn clone_service_map(map: &ServiceMap) -> ServiceMap {
    map.iter()
        .map(|(k, v)| {
            (k.clone(), UpstreamService {
                service: RwLock::new(v.service.read().clone()),
                ep_slice: RwLock::new(v.ep_slice.read().clone()),
            })
        })
        .collect()
}
