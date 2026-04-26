//! Global store for EdgionPlugins resources

use arc_swap::ArcSwap;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::LazyLock;

use crate::core::gateway::plugins::runtime::PluginRuntime;
use crate::types::resources::EdgionPlugins;

static GLOBAL_PLUGIN_STORE: LazyLock<Arc<PluginStore>> = LazyLock::new(|| Arc::new(PluginStore::new()));

pub fn get_global_plugin_store() -> Arc<PluginStore> {
    GLOBAL_PLUGIN_STORE.clone()
}

/// Type alias for the plugin map (key: namespace/name)
type PluginMap = HashMap<String, EdgionPlugins>;

pub struct PluginStore {
    plugins: ArcSwap<PluginMap>,
}

impl Default for PluginStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginStore {
    pub fn new() -> Self {
        Self {
            plugins: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// Check if a plugin exists
    pub fn contains(&self, key: &str) -> bool {
        let map = self.plugins.load();
        map.contains_key(key)
    }

    /// Get a plugin by key (namespace/name)
    pub fn get(&self, key: &str) -> Option<EdgionPlugins> {
        let map = self.plugins.load();
        map.get(key).cloned()
    }

    /// Execute a function with the plugin reference
    pub fn with_plugin<F, R>(&self, key: &str, f: F) -> Option<R>
    where
        F: FnOnce(&EdgionPlugins) -> R,
    {
        let map = self.plugins.load();
        map.get(key).map(f)
    }

    /// Get the plugin runtime Arc for a given key
    pub fn get_plugin_runtime(&self, key: &str) -> Option<Arc<PluginRuntime>> {
        self.with_plugin(key, |p| Arc::clone(&p.spec.plugin_runtime))
    }

    /// Replace all edgion_plugins atomically
    pub fn replace_all(&self, plugins: HashMap<String, EdgionPlugins>) {
        self.plugins.store(Arc::new(plugins));
    }

    /// Update edgion_plugins atomically (clone map + modify + swap)
    pub fn update(&self, add_or_update: HashMap<String, EdgionPlugins>, remove: &HashSet<String>) {
        let current = self.plugins.load();
        let current_map: &PluginMap = &current;
        let mut new_map: PluginMap = current_map.clone();

        for key in remove {
            new_map.remove(key);
        }
        for (key, plugin) in add_or_update {
            new_map.insert(key, plugin);
        }

        self.plugins.store(Arc::new(new_map));
    }

    /// Get total count of edgion_plugins
    pub fn count(&self) -> usize {
        let map = self.plugins.load();
        map.len()
    }

    /// Per-kind plugin entry count across all stored EdgionPlugins resources.
    ///
    /// Discriminator: `EdgionPlugin::type_name()` (enum variant name, e.g. "JwtAuth", "BasicAuth").
    /// Each EdgionPlugins resource may declare plugins across four stages:
    /// requestPlugins, upstreamResponseFilterPlugins, upstreamResponseBodyFilterPlugins,
    /// upstreamResponsePlugins. All stages are counted.
    pub fn count_by_kind(&self) -> BTreeMap<String, usize> {
        let mut out = BTreeMap::new();
        let map = self.plugins.load();
        for ep in map.values() {
            for entry in ep.request_filter_entries() {
                *out.entry(entry.plugin.type_name().to_string()).or_insert(0) += 1;
            }
            for entry in ep.upstream_response_filter_entries() {
                *out.entry(entry.plugin.type_name().to_string()).or_insert(0) += 1;
            }
            for entry in ep.upstream_response_body_filter_entries() {
                *out.entry(entry.plugin.type_name().to_string()).or_insert(0) += 1;
            }
            for entry in ep.upstream_response_entries() {
                *out.entry(entry.plugin.type_name().to_string()).or_insert(0) += 1;
            }
        }
        out
    }

    /// Sample resource keys (namespace/name) holding plugins of the given kind.
    #[allow(dead_code)] // Consumed by validator/sample-diagnostics (Task 15); allow until then.
    pub fn keys_by_kind(&self, kind: &str) -> Vec<String> {
        let mut out = Vec::new();
        let map = self.plugins.load();
        for (key, ep) in map.iter() {
            let has_kind = ep.request_filter_entries().iter().any(|e| e.plugin.type_name() == kind)
                || ep
                    .upstream_response_filter_entries()
                    .iter()
                    .any(|e| e.plugin.type_name() == kind)
                || ep
                    .upstream_response_body_filter_entries()
                    .iter()
                    .any(|e| e.plugin.type_name() == kind)
                || ep
                    .upstream_response_entries()
                    .iter()
                    .any(|e| e.plugin.type_name() == kind);
            if has_kind {
                out.push(key.clone());
            }
        }
        out
    }
}

#[cfg(test)]
mod kind_stats_tests {
    use super::*;

    #[test]
    fn empty_store_reports_empty_breakdown() {
        let store = PluginStore::new();
        let by_kind = store.count_by_kind();
        assert!(by_kind.is_empty());
        let keys = store.keys_by_kind("JwtAuth");
        assert!(keys.is_empty());
    }
}
