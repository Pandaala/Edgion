use crate::core::common::conf_sync::traits::{arc_map_to_owned, ConfHandler};
use edgion_resources::prelude_resources::GatewayClass;
use kube::ResourceExt;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Global store for GatewayClass resources (keyed by key_name())
static GATEWAY_CLASS_STORE: std::sync::LazyLock<RwLock<HashMap<String, GatewayClass>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

/// Count GatewayClass resources in the store
pub fn count_gateway_classes() -> usize {
    GATEWAY_CLASS_STORE.read().len()
}

/// Query GatewayClass by name (cluster-scoped: key == name)
#[allow(dead_code)]
pub fn get_gateway_class_by_name(name: &str) -> Option<GatewayClass> {
    GATEWAY_CLASS_STORE.read().get(name).cloned()
}

/// List all GatewayClass resources
#[allow(dead_code)]
pub fn list_gateway_classes() -> Vec<GatewayClass> {
    let mut items: Vec<GatewayClass> = GATEWAY_CLASS_STORE.read().values().cloned().collect();
    items.sort_by(|a, b| a.name_any().cmp(&b.name_any()));
    items
}

/// ConfHandler implementation for GatewayClass
pub struct GatewayClassHandler;

impl GatewayClassHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GatewayClassHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfHandler<GatewayClass> for GatewayClassHandler {
    fn full_set(&self, data: &HashMap<String, Arc<GatewayClass>>) {
        tracing::info!(
            count = data.len(),
            "GatewayClass full_set: received {} GatewayClass resources",
            data.len()
        );
        let mut store = GATEWAY_CLASS_STORE.write();
        *store = arc_map_to_owned(data);
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<GatewayClass>>,
        update: HashMap<String, Arc<GatewayClass>>,
        remove: HashSet<String>,
    ) {
        let mut store = GATEWAY_CLASS_STORE.write();

        if !add.is_empty() {
            tracing::info!(
                count = add.len(),
                "GatewayClass partial_update: added {} GatewayClass resources",
                add.len()
            );
            for (key, gc) in add {
                tracing::debug!(key = %key, name = %gc.name_any(), "GatewayClass added");
                store.insert(key, (*gc).clone());
            }
        }

        if !update.is_empty() {
            tracing::info!(
                count = update.len(),
                "GatewayClass partial_update: updated {} GatewayClass resources",
                update.len()
            );
            for (key, gc) in update {
                tracing::debug!(key = %key, name = %gc.name_any(), "GatewayClass updated");
                if store.insert(key.clone(), (*gc).clone()).is_none() {
                    tracing::warn!(key = %key, "GatewayClass update: key not found in store, inserting");
                }
            }
        }

        if !remove.is_empty() {
            tracing::info!(
                count = remove.len(),
                "GatewayClass partial_update: removed {} GatewayClass resources",
                remove.len()
            );
            for key in &remove {
                tracing::debug!(key = %key, "GatewayClass removed");
                if store.remove(key).is_none() {
                    tracing::debug!(key = %key, "GatewayClass remove: key not found in store");
                }
            }
        }
    }
}

/// Create GatewayClass handler
pub fn create_gateway_class_handler() -> Box<dyn ConfHandler<GatewayClass> + Send + Sync> {
    Box::new(GatewayClassHandler::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::gateway_class::GatewayClassSpec;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::collections::HashMap;

    static TEST_MUTEX: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    fn make_gateway_class(name: &str) -> GatewayClass {
        GatewayClass {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: GatewayClassSpec {
                controller_name: "test/controller".to_string(),
                description: None,
                parameters_ref: None,
            },
            status: None,
        }
    }

    fn clear_store() {
        GATEWAY_CLASS_STORE.write().clear();
    }

    #[test]
    fn test_full_set_replaces_existing() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = GatewayClassHandler::new();

        let mut data = HashMap::new();
        data.insert("foo".to_string(), Arc::new(make_gateway_class("foo")));
        data.insert("bar".to_string(), Arc::new(make_gateway_class("bar")));
        handler.full_set(&data);
        assert_eq!(count_gateway_classes(), 2);

        let mut data2 = HashMap::new();
        data2.insert("baz".to_string(), Arc::new(make_gateway_class("baz")));
        handler.full_set(&data2);
        assert_eq!(count_gateway_classes(), 1);
    }

    #[test]
    fn test_add_deduplicates() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = GatewayClassHandler::new();

        let mut add = HashMap::new();
        add.insert("foo".to_string(), Arc::new(make_gateway_class("foo")));
        handler.partial_update(add.clone(), HashMap::new(), std::collections::HashSet::new());
        handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());

        assert_eq!(count_gateway_classes(), 1, "duplicate add must not create two entries");
    }

    #[test]
    fn test_update_overwrites_existing() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = GatewayClassHandler::new();

        let mut add = HashMap::new();
        add.insert("foo".to_string(), Arc::new(make_gateway_class("foo")));
        handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());

        let mut updated = make_gateway_class("foo");
        updated.spec.controller_name = "updated/controller".to_string();
        let mut upd = HashMap::new();
        upd.insert("foo".to_string(), Arc::new(updated));
        handler.partial_update(HashMap::new(), upd, std::collections::HashSet::new());

        let result = get_gateway_class_by_name("foo").expect("must exist after update");
        assert_eq!(result.spec.controller_name, "updated/controller");
        assert_eq!(count_gateway_classes(), 1);
    }

    #[test]
    fn test_update_miss_inserts() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = GatewayClassHandler::new();

        let mut upd = HashMap::new();
        upd.insert("ghost".to_string(), Arc::new(make_gateway_class("ghost")));
        handler.partial_update(HashMap::new(), upd, std::collections::HashSet::new());

        assert_eq!(count_gateway_classes(), 1, "update miss must insert");
    }

    #[test]
    fn test_remove_works() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = GatewayClassHandler::new();

        let mut add = HashMap::new();
        add.insert("foo".to_string(), Arc::new(make_gateway_class("foo")));
        add.insert("bar".to_string(), Arc::new(make_gateway_class("bar")));
        handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());

        let mut remove = std::collections::HashSet::new();
        remove.insert("foo".to_string());
        handler.partial_update(HashMap::new(), HashMap::new(), remove);

        assert_eq!(count_gateway_classes(), 1);
        assert!(get_gateway_class_by_name("foo").is_none());
        assert!(get_gateway_class_by_name("bar").is_some());
    }

    #[test]
    fn test_remove_miss_does_not_panic() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = GatewayClassHandler::new();

        let mut remove = std::collections::HashSet::new();
        remove.insert("nonexistent".to_string());
        handler.partial_update(HashMap::new(), HashMap::new(), remove);
        assert_eq!(count_gateway_classes(), 0);
    }

    #[test]
    fn test_list_and_count() {
        let _g = TEST_MUTEX.lock();
        clear_store();
        let handler = GatewayClassHandler::new();

        let mut add = HashMap::new();
        add.insert("a".to_string(), Arc::new(make_gateway_class("a")));
        add.insert("b".to_string(), Arc::new(make_gateway_class("b")));
        add.insert("c".to_string(), Arc::new(make_gateway_class("c")));
        handler.partial_update(add, HashMap::new(), std::collections::HashSet::new());

        assert_eq!(count_gateway_classes(), 3);
        assert_eq!(list_gateway_classes().len(), 3);
    }
}
