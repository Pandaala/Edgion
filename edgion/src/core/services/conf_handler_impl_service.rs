use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use k8s_openapi::api::core::v1::Service;
use crate::core::conf_sync::traits::ConfHandler;
use crate::core::services::{ServiceMgr, UpstreamService, get_global_service_mgr};

/// Implement ConfHandler for Arc<ServiceMgr> to allow using the global instance
impl ConfHandler<Service> for Arc<ServiceMgr> {
    fn full_set(&self, data: &HashMap<String, Service>) {
        (**self).full_set(data)
    }

    fn partial_update(&self, add_or_update: HashMap<String, Service>, remove: HashSet<String>) {
        (**self).partial_update(add_or_update, remove)
    }
}

/// Create a ServiceMgr handler for registration with ConfigClient
/// Returns the global ServiceMgr instance
pub fn create_service_mgr_handler() -> Box<dyn ConfHandler<Service> + Send + Sync> {
    Box::new(get_global_service_mgr())
}

impl ConfHandler<Service> for ServiceMgr {
    /// Full set with a complete set of Services
    /// This is typically called during initial sync or re-list
    fn full_set(&self, data: &HashMap<String, Service>) {
        tracing::info!(component = "service_mgr", cnt = data.len(), "full set");
        let converted: HashMap<String, UpstreamService> = data
            .iter()
            .map(|(k, v)| (k.clone(), UpstreamService::with_service(v.clone())))
            .collect();
        self.replace_all(converted);
    }

    /// Handle partial configuration updates
    /// Processes additions, updates, and removals of Services
    fn partial_update(&self, add_or_update: HashMap<String, Service>, remove: HashSet<String>) {
        tracing::info!(
            component = "service_mgr",
            au = add_or_update.len(),
            rm = remove.len(),
            "partial update"
        );
        let converted: HashMap<String, UpstreamService> = add_or_update
            .into_iter()
            .map(|(k, v)| (k, UpstreamService::with_service(v)))
            .collect();
        self.update(converted, &remove);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_service(namespace: &str, name: &str) -> Service {
        let json = serde_json::json!({
            "apiVersion": "v1",
            "kind": "Service",
            "metadata": {
                "namespace": namespace,
                "name": name
            },
            "spec": {
                "ports": [{
                    "port": 80,
                    "targetPort": 8080
                }],
                "selector": {
                    "app": name
                }
            }
        });
        serde_json::from_value(json).expect("Failed to create Service")
    }

    #[test]
    fn test_full_set() {
        let mgr = ServiceMgr::new();
        
        let mut data = HashMap::new();
        data.insert("default/svc1".to_string(), create_test_service("default", "svc1"));
        data.insert("default/svc2".to_string(), create_test_service("default", "svc2"));
        
        mgr.full_set(&data);
        
        assert!(mgr.contains("default/svc1"));
        assert!(mgr.contains("default/svc2"));
        assert!(!mgr.contains("default/svc3"));
    }

    #[test]
    fn test_partial_update_add() {
        let mgr = ServiceMgr::new();
        
        let mut add_or_update = HashMap::new();
        add_or_update.insert("default/svc1".to_string(), create_test_service("default", "svc1"));
        
        mgr.partial_update(add_or_update, HashSet::new());
        
        assert!(mgr.contains("default/svc1"));
    }

    #[test]
    fn test_partial_update_remove() {
        let mgr = ServiceMgr::new();
        
        // First add a service
        let mut data = HashMap::new();
        data.insert("default/svc1".to_string(), create_test_service("default", "svc1"));
        mgr.full_set(&data);
        
        // Then remove it
        let mut remove = HashSet::new();
        remove.insert("default/svc1".to_string());
        mgr.partial_update(HashMap::new(), remove);
        
        assert!(!mgr.contains("default/svc1"));
    }
}

