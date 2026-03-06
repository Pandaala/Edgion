use super::{get_global_service_store, ServiceStore};
use crate::core::common::conf_sync::traits::ConfHandler;
use crate::core::gateway::backends::health::check::{
    annotation::parse_health_check_annotation, get_hc_config_store, get_health_check_manager,
};
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

        let config_store = get_hc_config_store();
        let hc_manager = get_health_check_manager();
        let incoming_keys: HashSet<String> = data.keys().cloned().collect();

        for old_key in config_store.service_keys() {
            if !incoming_keys.contains(&old_key) {
                config_store.set_service_config(&old_key, None);
                hc_manager.reconcile_service(&old_key);
            }
        }

        for (key, service) in data {
            let active_config = parse_health_check_annotation(&service.metadata);
            config_store.set_service_config(key, active_config);
            hc_manager.reconcile_service(key);
        }
    }

    fn partial_update(&self, add: HashMap<String, Service>, update: HashMap<String, Service>, remove: HashSet<String>) {
        tracing::info!(
            component = "service_store",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );

        let config_store = get_hc_config_store();
        let hc_manager = get_health_check_manager();

        for (key, service) in add.iter().chain(update.iter()) {
            let active_config = parse_health_check_annotation(&service.metadata);
            config_store.set_service_config(key, active_config);
            hc_manager.reconcile_service(key);
        }

        for key in &remove {
            config_store.set_service_config(key, None);
            hc_manager.reconcile_service(key);
        }

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
