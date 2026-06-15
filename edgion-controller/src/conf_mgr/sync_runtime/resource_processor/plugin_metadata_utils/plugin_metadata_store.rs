//! EdgionData controller store
//!
//! Provides a store for EdgionData resources accessible during EdgionPlugins processing.
//!
//! This is control-plane code with low read/write frequency,
//! so a simple RwLock<HashMap> is sufficient.

use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};

use edgion_resources::prelude_resources::EdgionData;

/// Store for EdgionData resources (controller-side, low-frequency access).
///
/// Key format: "namespace/name" (namespaced resources only).
pub struct PluginMetaDataStore {
    data: RwLock<HashMap<String, EdgionData>>,
}

impl PluginMetaDataStore {
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }

    /// Get an EdgionData by namespace and name.
    pub fn get(&self, namespace: Option<&str>, name: &str) -> Option<EdgionData> {
        let key = Self::make_key(namespace, name);
        let map = self.data.read();
        map.get(&key).cloned()
    }

    /// Replace all entries (used during reset).
    pub fn replace_all(&self, data: HashMap<String, EdgionData>) {
        let count = data.len();
        let mut map = self.data.write();
        *map = data;
        tracing::debug!(
            component = "edgion_data_store",
            count = count,
            "Replaced all EdgionData entries"
        );
    }

    /// Upsert/remove entries.
    ///
    /// - `upsert`: entries to add or update
    /// - `remove`: keys of entries to remove
    pub fn update(&self, upsert: HashMap<String, EdgionData>, remove: &HashSet<String>) {
        let mut map = self.data.write();
        for key in remove {
            map.remove(key);
        }
        for (key, ed) in upsert {
            map.insert(key, ed);
        }
        tracing::debug!(component = "edgion_data_store", "Updated EdgionData store");
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
