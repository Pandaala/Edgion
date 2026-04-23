//! PluginMetaData Store
//!
//! Provides a store for PluginMetaData resources accessible during EdgionPlugins processing.
//!
//! This is control-plane code with low read/write frequency,
//! so a simple RwLock<HashMap> is sufficient.

use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};

use crate::types::prelude_resources::PluginMetaData;

/// Store for PluginMetaData resources.
///
/// Key format: "namespace/name" (namespaced resources only).
pub struct PluginMetaDataStore {
    data: RwLock<HashMap<String, PluginMetaData>>,
}

impl PluginMetaDataStore {
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }

    /// Get a PluginMetaData by namespace and name.
    pub fn get(&self, namespace: Option<&str>, name: &str) -> Option<PluginMetaData> {
        let key = Self::make_key(namespace, name);
        let map = self.data.read();
        map.get(&key).cloned()
    }

    /// Replace all entries (used during reset).
    pub fn replace_all(&self, data: HashMap<String, PluginMetaData>) {
        let count = data.len();
        let mut map = self.data.write();
        *map = data;
        tracing::debug!(
            component = "plugin_metadata_store",
            count = count,
            "Replaced all PluginMetaData entries"
        );
    }

    /// Upsert/remove entries.
    ///
    /// - `upsert`: entries to add or update
    /// - `remove`: keys of entries to remove
    pub fn update(&self, upsert: HashMap<String, PluginMetaData>, remove: &HashSet<String>) {
        let mut map = self.data.write();
        for key in remove {
            map.remove(key);
        }
        for (key, pm) in upsert {
            map.insert(key, pm);
        }
        tracing::debug!(component = "plugin_metadata_store", "Updated PluginMetaData store");
    }

    /// Build a "namespace/name" key.
    pub fn make_key(namespace: Option<&str>, name: &str) -> String {
        match namespace {
            Some(ns) => format!("{}/{}", ns, name),
            None => name.to_string(),
        }
    }
}

impl Default for PluginMetaDataStore {
    fn default() -> Self {
        Self::new()
    }
}
