use crate::core::conf_sync::traits::ConfHandler;
use crate::types::prelude_resources::GatewayClass;
use kube::ResourceExt;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

/// Global store for GatewayClass resources
static GATEWAY_CLASS_STORE: std::sync::LazyLock<Arc<RwLock<Vec<GatewayClass>>>> =
    std::sync::LazyLock::new(|| Arc::new(RwLock::new(Vec::new())));

/// Get a reference to the global GatewayClass store
pub fn get_gateway_class_store() -> Arc<RwLock<Vec<GatewayClass>>> {
    GATEWAY_CLASS_STORE.clone()
}

/// Query GatewayClass by name
pub fn get_gateway_class_by_name(name: &str) -> Option<GatewayClass> {
    let store = GATEWAY_CLASS_STORE.read().unwrap();
    store.iter().find(|gc| gc.name_any() == name).cloned()
}

/// List all GatewayClass resources
pub fn list_gateway_classes() -> Vec<GatewayClass> {
    let store = GATEWAY_CLASS_STORE.read().unwrap();
    store.clone()
}

/// ConfHandler implementation for GatewayClass
/// Stores GatewayClass resources in cache for dynamic lookup
pub struct GatewayClassHandler;

impl GatewayClassHandler {
    pub fn new() -> Self {
        Self
    }
}

impl ConfHandler<GatewayClass> for GatewayClassHandler {
    fn full_set(&self, data: &HashMap<String, GatewayClass>) {
        tracing::info!(
            count = data.len(),
            "GatewayClass full_set: received {} GatewayClass resources",
            data.len()
        );

        // Update global store
        let mut store = GATEWAY_CLASS_STORE.write().unwrap();
        store.clear();
        for (key, gateway_class) in data {
            tracing::debug!(
                key = %key,
                name = %gateway_class.name_any(),
                "GatewayClass stored"
            );
            store.push(gateway_class.clone());
        }
    }

    fn partial_update(
        &self,
        add: HashMap<String, GatewayClass>,
        update: HashMap<String, GatewayClass>,
        remove: HashSet<String>,
    ) {
        let mut store = GATEWAY_CLASS_STORE.write().unwrap();

        if !add.is_empty() {
            tracing::info!(
                count = add.len(),
                "GatewayClass partial_update: added {} GatewayClass resources",
                add.len()
            );
            for (key, gateway_class) in add {
                tracing::info!(
                    key = %key,
                    name = %gateway_class.name_any(),
                    "GatewayClass added"
                );
                store.push(gateway_class);
            }
        }

        if !update.is_empty() {
            tracing::info!(
                count = update.len(),
                "GatewayClass partial_update: updated {} GatewayClass resources",
                update.len()
            );
            for (key, gateway_class) in update {
                tracing::info!(
                    key = %key,
                    name = %gateway_class.name_any(),
                    "GatewayClass updated"
                );
                // Find and update existing gateway_class
                if let Some(existing) = store.iter_mut().find(|gc| gc.name_any() == gateway_class.name_any()) {
                    *existing = gateway_class;
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
                tracing::info!(
                    key = %key,
                    "GatewayClass removed"
                );
            }
            // Remove gateway_classes whose key is in the remove set
            store.retain(|gc| !remove.contains(&gc.name_any()));
        }
    }
}

/// Create GatewayClass handler
pub fn create_gateway_class_handler() -> Box<dyn ConfHandler<GatewayClass>> {
    Box::new(GatewayClassHandler::new())
}
