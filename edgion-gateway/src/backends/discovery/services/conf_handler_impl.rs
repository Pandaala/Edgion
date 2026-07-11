use super::{get_global_service_store, ServiceStore};
use crate::lb::compiled_selector;
use crate::lb::runtime_state;
use edgion_common::conf_sync::traits::{arc_map_into_owned, arc_map_to_owned, ConfHandler};
use k8s_openapi::api::core::v1::Service;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Create a ServiceStore handler for registration with ConfigClient
pub fn create_service_handler() -> Box<dyn ConfHandler<Service> + Send + Sync> {
    Box::new(get_global_service_store())
}

impl ConfHandler<Service> for ServiceStore {
    fn full_set(&self, data: &HashMap<String, Arc<Service>>) {
        tracing::info!(component = "service_store", cnt = data.len(), "full set");

        // Snapshot the previous key set before the swap so we can drop per-backend
        // runtime state and selector cells for Services removed by this full set.
        // Per-service policy config (health check / circuit breaker / panic
        // threshold / LB policy) is owned exclusively by the EdgionBackendTrafficPolicy
        // conf-handler, so a Service resync must never touch those stores.
        let old_keys = self.keys();
        self.replace_all(arc_map_to_owned(data));

        let incoming_keys: HashSet<&String> = data.keys().collect();
        for old_key in &old_keys {
            if !incoming_keys.contains(old_key) {
                runtime_state::remove_service(old_key);
                compiled_selector::drop_cell(old_key);
            }
        }
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<Service>>,
        update: HashMap<String, Arc<Service>>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "service_store",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );

        // Per-service policy config is owned by the EdgionBackendTrafficPolicy
        // conf-handler; only per-backend runtime state and selector cells are
        // cleaned up here for removed Services.
        for key in &remove {
            runtime_state::remove_service(key);
            compiled_selector::drop_cell(key);
        }

        // Merge add and update for storage, extracting from Arc
        let mut add_or_update = arc_map_into_owned(add);
        add_or_update.extend(arc_map_into_owned(update));

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
        data.insert(
            "default/svc1".to_string(),
            Arc::new(create_test_service("default", "svc1")),
        );
        data.insert(
            "default/svc2".to_string(),
            Arc::new(create_test_service("default", "svc2")),
        );

        store.full_set(&data);

        assert!(store.contains("default/svc1"));
        assert!(store.contains("default/svc2"));
        assert!(!store.contains("default/svc3"));
    }

    #[test]
    fn test_partial_update_add() {
        let store = ServiceStore::new();

        let mut add = HashMap::new();
        add.insert(
            "default/svc1".to_string(),
            Arc::new(create_test_service("default", "svc1")),
        );

        store.partial_update(add, HashMap::new(), HashSet::new());

        assert!(store.contains("default/svc1"));
    }

    #[test]
    fn test_partial_update_update() {
        let store = ServiceStore::new();

        // First add a service
        let mut data = HashMap::new();
        data.insert(
            "default/svc1".to_string(),
            Arc::new(create_test_service("default", "svc1")),
        );
        store.full_set(&data);

        // Then update it
        let mut update = HashMap::new();
        update.insert(
            "default/svc1".to_string(),
            Arc::new(create_test_service("default", "svc1")),
        );

        store.partial_update(HashMap::new(), update, HashSet::new());

        assert!(store.contains("default/svc1"));
    }

    #[test]
    fn test_partial_update_remove() {
        let store = ServiceStore::new();

        // First add a service
        let mut data = HashMap::new();
        data.insert(
            "default/svc1".to_string(),
            Arc::new(create_test_service("default", "svc1")),
        );
        store.full_set(&data);

        // Then remove it
        let mut remove = HashSet::new();
        remove.insert("default/svc1".to_string());
        store.partial_update(HashMap::new(), HashMap::new(), remove);

        assert!(!store.contains("default/svc1"));
    }
}
