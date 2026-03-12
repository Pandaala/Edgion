//! Global store for EdgionPlugins resources

use arc_swap::ArcSwap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::LazyLock;

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
}
