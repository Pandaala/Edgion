//! Global store for EdgionStreamPlugins resources

use arc_swap::ArcSwap;
use std::collections::{HashMap, HashSet};
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
}

impl Default for StreamPluginStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfHandler<EdgionStreamPlugins> for Arc<StreamPluginStore> {
    fn full_set(&self, data: &HashMap<String, EdgionStreamPlugins>) {
        tracing::info!(component = "stream_plugin_store", cnt = data.len(), "full set");

        let plugins: HashMap<String, Arc<EdgionStreamPlugins>> =
            data.iter().map(|(k, v)| (k.clone(), Arc::new(v.clone()))).collect();

        self.replace_all(plugins);
    }

    fn partial_update(
        &self,
        add: HashMap<String, EdgionStreamPlugins>,
        update: HashMap<String, EdgionStreamPlugins>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "stream_plugin_store",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );

        // Combine add and update
        let mut add_or_update = HashMap::new();
        for (k, v) in add {
            tracing::debug!(key = %k, "Adding EdgionStreamPlugins");
            add_or_update.insert(k, Arc::new(v));
        }
        for (k, v) in update {
            tracing::debug!(key = %k, "Updating EdgionStreamPlugins");
            add_or_update.insert(k, Arc::new(v));
        }

        // Log removals
        for key in &remove {
            tracing::debug!(key = %key, "Removing EdgionStreamPlugins");
        }

        self.update(add_or_update, &remove);
    }
}

impl ConfHandler<EdgionStreamPlugins> for &'static StreamPluginStore {
    fn full_set(&self, data: &HashMap<String, EdgionStreamPlugins>) {
        tracing::info!(
            component = "stream_plugin_store",
            cnt = data.len(),
            "full set (static ref)"
        );

        let plugins: HashMap<String, Arc<EdgionStreamPlugins>> =
            data.iter().map(|(k, v)| (k.clone(), Arc::new(v.clone()))).collect();

        self.replace_all(plugins);
    }

    fn partial_update(
        &self,
        add: HashMap<String, EdgionStreamPlugins>,
        update: HashMap<String, EdgionStreamPlugins>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "stream_plugin_store",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update (static ref)"
        );

        // Combine add and update
        let mut add_or_update = HashMap::new();
        for (k, v) in add {
            tracing::debug!(key = %k, "Adding EdgionStreamPlugins (static ref)");
            add_or_update.insert(k, Arc::new(v));
        }
        for (k, v) in update {
            tracing::debug!(key = %k, "Updating EdgionStreamPlugins (static ref)");
            add_or_update.insert(k, Arc::new(v));
        }

        // Log removals
        for key in &remove {
            tracing::debug!(key = %key, "Removing EdgionStreamPlugins (static ref)");
        }

        self.update(add_or_update, &remove);
    }
}
