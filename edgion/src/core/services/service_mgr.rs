use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use k8s_openapi::api::core::v1::Service;

static GLOBAL_SERVICE_MGR: Lazy<Arc<ServiceMgr>> = 
    Lazy::new(|| Arc::new(ServiceMgr::new()));

pub fn get_global_service_mgr() -> Arc<ServiceMgr> {
    GLOBAL_SERVICE_MGR.clone()
}

pub struct ServiceMgr {
    services: DashMap<String, Service>,
}

impl ServiceMgr {
    pub fn new() -> ServiceMgr {
        Self { services: DashMap::new() }
    }

    pub fn get(&self, key: &str) -> Option<Service> {
        self.services.get(key).map(|r| r.value().clone())
    }

    pub fn replace_all(&self, services: HashMap<String, Service>) {
        self.services.clear();
        for (key, service) in services {
            self.services.insert(key, service);
        }
    }

    pub fn update(&self, add_or_update: HashMap<String, Service>, remove: &HashSet<String>) {
        for key in remove {
            self.services.remove(key);
        }
        for (key, service) in add_or_update {
            self.services.insert(key, service);
        }
    }
}
