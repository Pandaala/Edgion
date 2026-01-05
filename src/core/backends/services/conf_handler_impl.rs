use super::{get_global_service_store, ServiceStore};
use crate::core::conf_sync::traits::ConfHandler;
use k8s_openapi::api::core::v1::Service;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Implement ConfHandler for Arc<ServiceStore>
impl ConfHandler<Service> for Arc<ServiceStore> {
    fn full_set(&self, data: &HashMap<String, Service>) {
        (**self).full_set(data)
    }

    fn partial_update(&self, add: HashMap<String, Service>, update: HashMap<String, Service>, remove: HashSet<String>) {
        (**self).partial_update(add, update, remove)
    }
}

/// Create a ServiceStore handler for registration with ConfigClient
pub fn create_service_handler() -> Box<dyn ConfHandler<Service> + Send + Sync> {
    Box::new(get_global_service_store())
}

impl ConfHandler<Service> for ServiceStore {
    fn full_set(&self, data: &HashMap<String, Service>) {
        tracing::info!(component = "service_store", cnt = data.len(), "full set");
        self.replace_all(data.clone());
    }

    fn partial_update(&self, add: HashMap<String, Service>, update: HashMap<String, Service>, remove: HashSet<String>) {
        tracing::info!(
            component = "service_store",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );

        // Merge add and update for storage
        let mut add_or_update = add;
        add_or_update.extend(update);

        self.update(add_or_update, &remove);
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
        let store = ServiceStore::new();

        let mut data = HashMap::new();
        data.insert("default/svc1".to_string(), create_test_service("default", "svc1"));
        data.insert("default/svc2".to_string(), create_test_service("default", "svc2"));

        store.full_set(&data);

        assert!(store.contains("default/svc1"));
        assert!(store.contains("default/svc2"));
        assert!(!store.contains("default/svc3"));
    }

    #[test]
    fn test_partial_update_add() {
        let store = ServiceStore::new();

        let mut add = HashMap::new();
        add.insert("default/svc1".to_string(), create_test_service("default", "svc1"));

        store.partial_update(add, HashMap::new(), HashSet::new());

        assert!(store.contains("default/svc1"));
    }

    #[test]
    fn test_partial_update_update() {
        let store = ServiceStore::new();

        // First add a service
        let mut data = HashMap::new();
        data.insert("default/svc1".to_string(), create_test_service("default", "svc1"));
        store.full_set(&data);

        // Then update it
        let mut update = HashMap::new();
        update.insert("default/svc1".to_string(), create_test_service("default", "svc1"));

        store.partial_update(HashMap::new(), update, HashSet::new());

        assert!(store.contains("default/svc1"));
    }

    #[test]
    fn test_partial_update_remove() {
        let store = ServiceStore::new();

        // First add a service
        let mut data = HashMap::new();
        data.insert("default/svc1".to_string(), create_test_service("default", "svc1"));
        store.full_set(&data);

        // Then remove it
        let mut remove = HashSet::new();
        remove.insert("default/svc1".to_string());
        store.partial_update(HashMap::new(), HashMap::new(), remove);

        assert!(!store.contains("default/svc1"));
    }
}
