//! Global store for EdgionStreamPlugins resources

use arc_swap::ArcSwap;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::LazyLock;

use crate::core::common::conf_sync::traits::ConfHandler;
use crate::types::resources::EdgionStreamPlugins;

static GLOBAL_STREAM_PLUGIN_STORE: LazyLock<Arc<StreamPluginStore>> =
    LazyLock::new(|| Arc::new(StreamPluginStore::new()));

pub fn get_global_stream_plugin_store() -> Arc<StreamPluginStore> {
    GLOBAL_STREAM_PLUGIN_STORE.clone()
}

/// Create a handler for EdgionStreamPlugins
pub fn create_stream_plugin_handler() -> Box<dyn ConfHandler<EdgionStreamPlugins> + Send + Sync> {
    Box::new(get_global_stream_plugin_store())
}

/// Type alias for the stream plugin map (key: namespace/name)
type StreamPluginMap = HashMap<String, Arc<EdgionStreamPlugins>>;

pub struct StreamPluginStore {
    plugins: ArcSwap<StreamPluginMap>,
}

impl StreamPluginStore {
    pub fn new() -> Self {
        Self {
            plugins: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Check if a stream plugin exists
    pub fn contains(&self, key: &str) -> bool {
        let map = self.plugins.load();
        map.contains_key(key)
    }

    /// Get a stream plugin by key (namespace/name)
    pub fn get(&self, key: &str) -> Option<Arc<EdgionStreamPlugins>> {
        let map = self.plugins.load();
        map.get(key).cloned()
    }

    /// Get a stream plugin by namespace and name
    pub fn get_by_ns_name(&self, namespace: &str, name: &str) -> Option<Arc<EdgionStreamPlugins>> {
        let key = format!("{}/{}", namespace, name);
        self.get(&key)
    }

    /// Execute a function with the plugin reference
    pub fn with_plugin<F, R>(&self, key: &str, f: F) -> Option<R>
    where
        F: FnOnce(&EdgionStreamPlugins) -> R,
    {
        let map = self.plugins.load();
        map.get(key).map(|p| f(p))
    }

    /// Replace all stream plugins atomically
    pub fn replace_all(&self, plugins: HashMap<String, Arc<EdgionStreamPlugins>>) {
        self.plugins.store(Arc::new(plugins));
    }

    /// Update stream plugins atomically (clone map + modify + swap)
    pub fn update(&self, add_or_update: HashMap<String, Arc<EdgionStreamPlugins>>, remove: &HashSet<String>) {
        let current = self.plugins.load();
        let mut new_map = (**current).clone();

        // Add or update plugins
        for (key, plugin) in add_or_update {
            new_map.insert(key, plugin);
        }

        // Remove plugins
        for key in remove {
            new_map.remove(key);
        }

        self.plugins.store(Arc::new(new_map));
    }

    /// Get all plugins
    pub fn get_all(&self) -> Arc<StreamPluginMap> {
        self.plugins.load_full()
    }

    /// Get total count of stream plugins
    pub fn count(&self) -> usize {
        self.plugins.load().len()
    }

    /// Per-kind plugin entry count across all stored EdgionStreamPlugins resources.
    ///
    /// Discriminator: `EdgionStreamPlugin::type_name()` / `TlsRouteStreamPlugin::type_name()`
    /// (enum variant name, e.g. "IpRestriction", "GlobalConnectionIpRestriction").
    /// Both Stage 1 (ConnectionFilter) and Stage 2 (TlsRoute) entries are counted.
    pub fn count_by_kind(&self) -> BTreeMap<String, usize> {
        let mut out = BTreeMap::new();
        let map = self.plugins.load();
        for esp in map.values() {
            if let Some(entries) = &esp.spec.plugins {
                for entry in entries {
                    *out.entry(entry.plugin.type_name().to_string()).or_insert(0) += 1;
                }
            }
            if let Some(entries) = &esp.spec.tls_route_plugins {
                for entry in entries {
                    *out.entry(entry.plugin.type_name().to_string()).or_insert(0) += 1;
                }
            }
        }
        out
    }

    /// Sample resource keys (namespace/name) holding stream plugins of the given kind.
    #[allow(dead_code)] // Consumed by validator/sample-diagnostics (Task 15); allow until then.
    pub fn keys_by_kind(&self, kind: &str) -> Vec<String> {
        let mut out = Vec::new();
        let map = self.plugins.load();
        for (key, esp) in map.iter() {
            let in_stage1 = esp
                .spec
                .plugins
                .as_ref()
                .is_some_and(|entries| entries.iter().any(|e| e.plugin.type_name() == kind));
            let in_stage2 = esp
                .spec
                .tls_route_plugins
                .as_ref()
                .is_some_and(|entries| entries.iter().any(|e| e.plugin.type_name() == kind));
            if in_stage1 || in_stage2 {
                out.push(key.clone());
            }
        }
        out
    }
}

impl Default for StreamPluginStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfHandler<EdgionStreamPlugins> for StreamPluginStore {
    fn full_set(&self, data: &HashMap<String, Arc<EdgionStreamPlugins>>) {
        tracing::info!(component = "stream_plugin_store", cnt = data.len(), "full set");
        let plugins: HashMap<String, Arc<EdgionStreamPlugins>> =
            data.iter().map(|(k, v)| (k.clone(), Arc::clone(v))).collect();
        self.replace_all(plugins);
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<EdgionStreamPlugins>>,
        update: HashMap<String, Arc<EdgionStreamPlugins>>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "stream_plugin_store",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );
        let mut add_or_update: HashMap<String, Arc<EdgionStreamPlugins>> = add;
        add_or_update.extend(update);
        for key in &remove {
            tracing::debug!(key = %key, "Removing EdgionStreamPlugins");
        }
        self.update(add_or_update, &remove);
    }
}

#[cfg(test)]
mod kind_stats_tests {
    use super::*;

    #[test]
    fn empty_store_reports_empty_breakdown() {
        let store = StreamPluginStore::new();
        let by_kind = store.count_by_kind();
        assert!(by_kind.is_empty());
        let keys = store.keys_by_kind("IpRestriction");
        assert!(keys.is_empty());
    }
}
