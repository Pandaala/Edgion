use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::prelude_resources::GatewayClass;
use kube::ResourceExt;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[cfg(not(test))]
use crate::{ai::reconcile_all_adaptive_plugin_dependents, plugins::publication::global_publisher};
#[cfg(test)]
use tests::{global_publisher, reconcile_all_adaptive_plugin_dependents};

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
        global_publisher().replace_gateway_classes(data.clone());
        reconcile_all_adaptive_plugin_dependents();
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

        if !add.is_empty() {
            tracing::info!(
                count = add.len(),
                "GatewayClass partial_update: added {} GatewayClass resources",
                add.len()
            );
            for (key, gc) in add {
                tracing::debug!(key = %key, name = %gc.name_any(), "GatewayClass added");
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
            }
        }
        global_publisher().update_gateway_classes(policy_inputs, &remove);
        reconcile_all_adaptive_plugin_dependents();
    }
}

/// Create GatewayClass handler
pub fn create_gateway_class_handler() -> Box<dyn ConfHandler<GatewayClass> + Send + Sync> {
    Box::new(GatewayClassHandler::new())
}

#[cfg(test)]
#[path = "../../../tests/unit/config/gateway_class/conf_handler_impl_test.rs"]
mod tests;
