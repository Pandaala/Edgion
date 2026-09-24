use edgion_common::conf_sync::traits::{arc_map_to_owned, ConfHandler};
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

#[cfg(test)]
include!("../../../tests/support/config/gateway_class/conf_handler_impl_test/20_function_item.rs");

#[cfg(test)]
include!("../../../tests/support/config/gateway_class/conf_handler_impl_test/28_function_item.rs");

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
        drop(store);
        crate::plugins::publication::global_publisher().replace_gateway_classes(data.clone());
        crate::ai::reconcile_all_adaptive_plugin_dependents();
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<GatewayClass>>,
        update: HashMap<String, Arc<GatewayClass>>,
        remove: HashSet<String>,
    ) {
        let policy_inputs = add
            .iter()
            .chain(update.iter())
            .map(|(key, value)| (key.clone(), Arc::clone(value)))
            .collect();
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
        drop(store);
        crate::plugins::publication::global_publisher().update_gateway_classes(policy_inputs, &remove);
        crate::ai::reconcile_all_adaptive_plugin_dependents();
    }
}

/// Create GatewayClass handler
pub fn create_gateway_class_handler() -> Box<dyn ConfHandler<GatewayClass> + Send + Sync> {
    Box::new(GatewayClassHandler::new())
}

#[cfg(test)]
#[path = "../../../tests/unit/config/gateway_class/conf_handler_impl_test.rs"]
mod tests;
