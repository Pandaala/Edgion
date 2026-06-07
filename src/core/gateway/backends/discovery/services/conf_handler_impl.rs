use super::{get_global_service_store, ServiceStore};
use crate::core::common::conf_sync::traits::{arc_map_into_owned, arc_map_to_owned, ConfHandler};
use crate::core::gateway::backends::circuit_breaker::{
    config::parse_circuit_breaker_annotation, get_circuit_breaker_store,
};
use crate::core::gateway::backends::health::check::{
    annotation::parse_health_check_annotation, get_hc_config_store, get_health_check_manager,
};
use crate::core::gateway::backends::panic_threshold::{get_panic_threshold_store, parse_panic_threshold_annotation};
use crate::core::gateway::lb::compiled_selector;
use crate::core::gateway::lb::runtime_state;
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
        self.replace_all(arc_map_to_owned(data));

        let config_store = get_hc_config_store();
        let hc_manager = get_health_check_manager();
        let cb_store = get_circuit_breaker_store();
        let panic_store = get_panic_threshold_store();
        let incoming_keys: HashSet<String> = data.keys().cloned().collect();

        for old_key in config_store.service_keys() {
            if !incoming_keys.contains(&old_key) {
                config_store.set_service_config(&old_key, None);
                hc_manager.reconcile_service(&old_key);
                runtime_state::remove_service(&old_key);
                cb_store.remove_service(&old_key);
                panic_store.remove_service(&old_key);
                compiled_selector::drop_cell(&old_key);
            }
        }

        for (key, service) in data {
            let active_config = parse_health_check_annotation(&service.metadata);
            config_store.set_service_config(key, active_config);
            hc_manager.reconcile_service(key);
            let cb_config = parse_circuit_breaker_annotation(&service.metadata);
            cb_store.set_service_config(key, cb_config);
            panic_store.set_service_threshold(key, parse_panic_threshold_annotation(&service.metadata));
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

        let config_store = get_hc_config_store();
        let hc_manager = get_health_check_manager();
        let cb_store = get_circuit_breaker_store();
        let panic_store = get_panic_threshold_store();

        for (key, service) in add.iter().chain(update.iter()) {
            let active_config = parse_health_check_annotation(&service.metadata);
            config_store.set_service_config(key, active_config);
            hc_manager.reconcile_service(key);
            let cb_config = parse_circuit_breaker_annotation(&service.metadata);
            cb_store.set_service_config(key, cb_config);
            panic_store.set_service_threshold(key, parse_panic_threshold_annotation(&service.metadata));
        }

        for key in &remove {
            config_store.set_service_config(key, None);
            hc_manager.reconcile_service(key);
            runtime_state::remove_service(key);
            cb_store.remove_service(key);
            panic_store.remove_service(key);
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

    #[test]
    fn test_panic_threshold_annotation_plumbed_and_removed() {
        use crate::core::gateway::backends::panic_threshold::{
            configure_lb_panic_default, get_panic_threshold_store, resolve_panic_threshold,
        };
        configure_lb_panic_default(0);
        let store = ServiceStore::new();

        let mut svc = create_test_service("default", "pt-svc");
        svc.metadata.annotations = Some(std::collections::BTreeMap::from([(
            "edgion.io/panic-threshold".to_string(),
            "40".to_string(),
        )]));

        let mut data = HashMap::new();
        data.insert("default/pt-svc".to_string(), Arc::new(svc));
        store.full_set(&data);
        assert_eq!(resolve_panic_threshold("default/pt-svc"), 40);

        // Removal clears the override (falls back to global default 0).
        store.partial_update(
            HashMap::new(),
            HashMap::new(),
            HashSet::from(["default/pt-svc".to_string()]),
        );
        assert_eq!(resolve_panic_threshold("default/pt-svc"), 0);
        get_panic_threshold_store().remove_service("default/pt-svc");
    }
}
