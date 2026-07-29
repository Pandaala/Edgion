//! ConfHandler implementation for EdgionPlugins

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::{compile_plugins, get_global_plugin_store, CompiledPlugins, PluginStore};
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::resources::EdgionPlugins;

/// Create a PluginStore handler for registration with ConfigClient
pub fn create_plugin_handler() -> Box<dyn ConfHandler<EdgionPlugins> + Send + Sync> {
    Box::new(get_global_plugin_store())
}

/// Compile every incoming EdgionPlugins into its gateway runtime companion,
/// keyed by the same map key. The runtime is built once per (ns/name) here.
fn compile_map(data: HashMap<String, Arc<EdgionPlugins>>) -> HashMap<String, Arc<CompiledPlugins>> {
    data.into_iter().map(|(k, v)| (k, compile_plugins(&v))).collect()
}

impl ConfHandler<EdgionPlugins> for PluginStore {
    fn full_set(&self, data: &HashMap<String, Arc<EdgionPlugins>>) {
        tracing::info!(component = "plugin_store", cnt = data.len(), "full set");
        let compiled = data.iter().map(|(k, v)| (k.clone(), compile_plugins(v))).collect();
        self.replace_all(compiled);
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<EdgionPlugins>>,
        update: HashMap<String, Arc<EdgionPlugins>>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "plugin_store",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );

        let mut add_or_update = compile_map(add);
        add_or_update.extend(compile_map(update));

        self.update(add_or_update, &remove);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::edgion_plugins::{EdgionPlugin, EdgionPluginsSpec, RequestFilterEntry};
    use edgion_resources::resources::http_route::HTTPHeaderFilter;

    fn create_test_plugin(namespace: &str, name: &str) -> EdgionPlugins {
        EdgionPlugins {
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
                ..Default::default()
            },
            status: None,
        }
    }

    fn create_invalid_test_plugin(namespace: &str, name: &str) -> EdgionPlugins {
        EdgionPlugins {
            metadata: kube::core::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: EdgionPluginsSpec {
                request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::ResponseHeaderModifier(
                    HTTPHeaderFilter {
                        set: None,
                        add: None,
                        remove: None,
                    },
                ))]),
                ..Default::default()
            },
            status: None,
        }
    }

    fn create_invalid_dsl_test_plugin(namespace: &str, name: &str) -> EdgionPlugins {
        use edgion_resources::resources::edgion_plugins::DslConfig;

        EdgionPlugins {
            metadata: kube::core::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: EdgionPluginsSpec {
                request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::Dsl(DslConfig {
                    name: "invalid-reload".to_string(),
                    source: Some("let value =".to_string()),
                    ..Default::default()
                }))]),
                ..Default::default()
            },
            status: None,
        }
    }

    #[test]
    fn test_full_set() {
        let store = PluginStore::new();

        let mut data = HashMap::new();
        data.insert(
            "default/plugin1".to_string(),
            Arc::new(create_test_plugin("default", "plugin1")),
        );
        data.insert(
            "default/plugin2".to_string(),
            Arc::new(create_test_plugin("default", "plugin2")),
        );

        store.full_set(&data);

        assert!(store.contains("default/plugin1"));
        assert!(store.contains("default/plugin2"));
        assert!(!store.contains("default/plugin3"));
    }

    #[test]
    fn test_partial_update_add() {
        let store = PluginStore::new();

        let mut add = HashMap::new();
        add.insert(
            "default/plugin1".to_string(),
            Arc::new(create_test_plugin("default", "plugin1")),
        );

        store.partial_update(add, HashMap::new(), HashSet::new());

        assert!(store.contains("default/plugin1"));
    }

    #[test]
    fn test_partial_update_update() {
        let store = PluginStore::new();

        let mut data = HashMap::new();
        data.insert(
            "default/plugin1".to_string(),
            Arc::new(create_test_plugin("default", "plugin1")),
        );
        store.full_set(&data);

        let mut update = HashMap::new();
        update.insert(
            "default/plugin1".to_string(),
            Arc::new(create_test_plugin("default", "plugin1")),
        );

        store.partial_update(HashMap::new(), update, HashSet::new());

        assert!(store.contains("default/plugin1"));
    }

    #[test]
    fn test_partial_update_remove() {
        let store = PluginStore::new();

        let mut data = HashMap::new();
        data.insert(
            "default/plugin1".to_string(),
            Arc::new(create_test_plugin("default", "plugin1")),
        );
        store.full_set(&data);

        let mut remove = HashSet::new();
        remove.insert("default/plugin1".to_string());
        store.partial_update(HashMap::new(), HashMap::new(), remove);

        assert!(!store.contains("default/plugin1"));
    }

    #[test]
    fn invalid_partial_update_retains_last_known_good() {
        let store = PluginStore::new();
        store.partial_update(
            HashMap::from([(
                "default/plugin1".to_string(),
                Arc::new(create_test_plugin("default", "plugin1")),
            )]),
            HashMap::new(),
            HashSet::new(),
        );

        store.partial_update(
            HashMap::new(),
            HashMap::from([(
                "default/plugin1".to_string(),
                Arc::new(create_invalid_dsl_test_plugin("default", "plugin1")),
            )]),
            HashSet::new(),
        );

        assert_eq!(
            store.with_plugin("default/plugin1", |compiled| compiled.is_activatable),
            Some(true)
        );
    }

    #[test]
    fn invalid_full_snapshot_retains_last_known_good_but_drops_absent_keys() {
        let store = PluginStore::new();
        store.full_set(&HashMap::from([
            (
                "default/kept".to_string(),
                Arc::new(create_test_plugin("default", "kept")),
            ),
            (
                "default/removed".to_string(),
                Arc::new(create_test_plugin("default", "removed")),
            ),
        ]));

        store.full_set(&HashMap::from([(
            "default/kept".to_string(),
            Arc::new(create_invalid_dsl_test_plugin("default", "kept")),
        )]));

        assert_eq!(
            store.with_plugin("default/kept", |compiled| compiled.is_activatable),
            Some(true)
        );
        assert!(!store.contains("default/removed"));
    }

    #[test]
    fn invalid_initial_resource_remains_fail_closed() {
        let store = PluginStore::new();
        store.full_set(&HashMap::from([(
            "default/plugin1".to_string(),
            Arc::new(create_invalid_test_plugin("default", "plugin1")),
        )]));

        assert_eq!(
            store.with_plugin("default/plugin1", |compiled| compiled.is_activatable),
            Some(false)
        );
    }

    #[test]
    fn non_dsl_invalid_update_replaces_last_known_good_with_fail_closed_runtime() {
        let store = PluginStore::new();
        store.full_set(&HashMap::from([(
            "default/plugin1".to_string(),
            Arc::new(create_test_plugin("default", "plugin1")),
        )]));

        store.partial_update(
            HashMap::new(),
            HashMap::from([(
                "default/plugin1".to_string(),
                Arc::new(create_invalid_test_plugin("default", "plugin1")),
            )]),
            HashSet::new(),
        );

        assert_eq!(
            store.with_plugin("default/plugin1", |compiled| compiled.is_activatable),
            Some(false)
        );
    }
}
