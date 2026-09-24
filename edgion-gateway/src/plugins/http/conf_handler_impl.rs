//! ConfHandler implementation for EdgionPlugins

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::{get_global_plugin_store, PluginStore};
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::resources::EdgionPlugins;

/// Create a PluginStore handler for registration with ConfigClient
pub fn create_plugin_handler() -> Box<dyn ConfHandler<EdgionPlugins> + Send + Sync> {
    Box::new(get_global_plugin_store())
}

impl ConfHandler<EdgionPlugins> for PluginStore {
    fn full_set(&self, data: &HashMap<String, Arc<EdgionPlugins>>) {
        #[cfg(test)]
        self.prepare_test_scope(data.keys().cloned().collect());
        self.publisher().replace_http_plugins(data.clone());
        if std::ptr::eq(self, super::global_plugin_store()) {
            crate::ai::reconcile_all_adaptive_plugin_dependents();
        }
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<EdgionPlugins>>,
        update: HashMap<String, Arc<EdgionPlugins>>,
        remove: HashSet<String>,
    ) {
        let changed_keys: HashSet<_> = add.keys().chain(update.keys()).chain(remove.iter()).cloned().collect();
        #[cfg(test)]
        {
            let mut keys: HashSet<_> = self.publisher().snapshot().http_sources().keys().cloned().collect();
            keys.extend(add.keys().chain(update.keys()).cloned());
            keys.retain(|key| !remove.contains(key));
            self.prepare_test_scope(keys);
        }
        self.publisher().update_http_plugins(add, update, &remove);
        if std::ptr::eq(self, super::global_plugin_store()) {
            crate::ai::reconcile_adaptive_plugin_keys(&changed_keys);
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/plugins/http/conf_handler_impl_test.rs"]
mod tests;
