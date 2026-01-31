//! ConfHandler implementation for EdgionPlugins

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::{get_global_plugin_store, PluginStore};
use crate::core::conf_sync::traits::ConfHandler;
use crate::types::resources::EdgionPlugins;

/// Implement ConfHandler for Arc<PluginStore>
impl ConfHandler<EdgionPlugins> for Arc<PluginStore> {
    fn full_set(&self, data: &HashMap<String, EdgionPlugins>) {
        (**self).full_set(data)
    }

    fn partial_update(
        &self,
        add: HashMap<String, EdgionPlugins>,
        update: HashMap<String, EdgionPlugins>,
        remove: HashSet<String>,
    ) {
        (**self).partial_update(add, update, remove)
    }
}

/// Create a PluginStore handler for registration with ConfigClient
pub fn create_plugin_handler() -> Box<dyn ConfHandler<EdgionPlugins> + Send + Sync> {
    Box::new(get_global_plugin_store())
}

impl ConfHandler<EdgionPlugins> for PluginStore {
    fn full_set(&self, data: &HashMap<String, EdgionPlugins>) {
        tracing::info!(component = "plugin_store", cnt = data.len(), "full set");
        self.replace_all(data.clone());
    }

    fn partial_update(
        &self,
        add: HashMap<String, EdgionPlugins>,
        update: HashMap<String, EdgionPlugins>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "plugin_store",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );

        let mut add_or_update = add;
        add_or_update.extend(update);

        self.update(add_or_update, &remove);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::edgion_plugins::{EdgionPlugin, EdgionPluginsSpec, RequestFilterEntry};
    use crate::types::resources::http_route::HTTPHeaderFilter;

    fn create_test_plugin(namespace: &str, name: &str) -> EdgionPlugins {
        let mut plugin = EdgionPlugins {
            metadata: kube::core::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: EdgionPluginsSpec {
                request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(
                    HTTPHeaderFilter {
                        set: None,
                        add: None,
                        remove: Some(vec!["X-Remove".to_string()]),
                    },
                ))]),
                upstream_response_filter_plugins: None,
                upstream_response_plugins: None,
                plugin_runtime: Default::default(),
            },
            status: None,
        };
        plugin.preparse();
        plugin
    }

    #[test]
    fn test_full_set() {
        let store = PluginStore::new();

        let mut data = HashMap::new();
        data.insert("default/plugin1".to_string(), create_test_plugin("default", "plugin1"));
        data.insert("default/plugin2".to_string(), create_test_plugin("default", "plugin2"));

        store.full_set(&data);

        assert!(store.contains("default/plugin1"));
        assert!(store.contains("default/plugin2"));
        assert!(!store.contains("default/plugin3"));
    }

    #[test]
    fn test_partial_update_add() {
        let store = PluginStore::new();

        let mut add = HashMap::new();
        add.insert("default/plugin1".to_string(), create_test_plugin("default", "plugin1"));

        store.partial_update(add, HashMap::new(), HashSet::new());

        assert!(store.contains("default/plugin1"));
    }

    #[test]
    fn test_partial_update_update() {
        let store = PluginStore::new();

        // First add a plugin
        let mut data = HashMap::new();
        data.insert("default/plugin1".to_string(), create_test_plugin("default", "plugin1"));
        store.full_set(&data);

        // Then update it
        let mut update = HashMap::new();
        update.insert("default/plugin1".to_string(), create_test_plugin("default", "plugin1"));

        store.partial_update(HashMap::new(), update, HashSet::new());

        assert!(store.contains("default/plugin1"));
    }

    #[test]
    fn test_partial_update_remove() {
        let store = PluginStore::new();

        // First add a plugin
        let mut data = HashMap::new();
        data.insert("default/plugin1".to_string(), create_test_plugin("default", "plugin1"));
        store.full_set(&data);

        // Then remove it
        let mut remove = HashSet::new();
        remove.insert("default/plugin1".to_string());
        store.partial_update(HashMap::new(), HashMap::new(), remove);

        assert!(!store.contains("default/plugin1"));
    }
}
