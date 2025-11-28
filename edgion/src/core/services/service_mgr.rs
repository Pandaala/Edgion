use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use k8s_openapi::api::core::v1::Service;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use parking_lot::RwLock;
use pingora_core::protocols::l4::socket::SocketAddr;
use crate::types::{HTTPBackendRef, MatchInfo};

static GLOBAL_SERVICE_MGR: Lazy<Arc<ServiceMgr>> =
    Lazy::new(|| Arc::new(ServiceMgr::new()));

pub fn get_global_service_mgr() -> Arc<ServiceMgr> {
    GLOBAL_SERVICE_MGR.clone()
}

pub struct UpstreamService {
    pub service: RwLock<Option<Service>>,
    pub ep_slice: RwLock<Option<EndpointSlice>>,
    // ep_lb:
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

pub struct ServiceMgr {
    services: DashMap<String, Arc<UpstreamService>>,
}

impl ServiceMgr {
    pub fn new() -> ServiceMgr {
        Self { services: DashMap::new() }
    }

    pub fn get_peer(&self, match_info: &MatchInfo, br: &HTTPBackendRef) -> Option<SocketAddr> {
        
        None
    }

    pub fn get(&self, key: &str) -> Option<Arc<UpstreamService>> {
        self.services.get(key).map(|r| r.value().clone())
    }

    pub fn get_or_create(&self, key: &str) -> Arc<UpstreamService> {
        self.services
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(UpstreamService::new()))
            .clone()
    }

    pub fn replace_all(&self, services: HashMap<String, Arc<UpstreamService>>) {
        self.services.clear();
        for (key, service) in services {
            self.services.insert(key, service);
        }
    }

    pub fn update(&self, add_or_update: HashMap<String, Arc<UpstreamService>>, remove: &HashSet<String>) {
        for key in remove {
            self.services.remove(key);
        }
        for (key, service) in add_or_update {
            self.services.insert(key, service);
        }
    }

    /// Update EndpointSlice for a service
    pub fn set_ep_slice(&self, service_key: &str, ep_slice: EndpointSlice) {
        let upstream = self.get_or_create(service_key);
        *upstream.ep_slice.write() = Some(ep_slice);
    }

    /// Remove EndpointSlice for a service
    pub fn remove_ep_slice(&self, service_key: &str) {
        if let Some(upstream) = self.get(service_key) {
            *upstream.ep_slice.write() = None;
        }
    }
}
