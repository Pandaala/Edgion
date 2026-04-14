//! PluginMetaData Reference Manager
//!
//! Tracks which EdgionPlugins/EdgionStreamPlugins resources reference a given PluginMetaData.
//! Used by PluginMetaData handler to know which resources to requeue when a PluginMetaData changes.

use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};

/// Tracks which EdgionPlugins/EdgionStreamPlugins resources reference a given PluginMetaData.
///
/// Key: `"namespace/pm_name"` -> Set of `"Kind:namespace/ep_name"` referencing it.
pub struct PluginMetaDataRefManager {
    refs: RwLock<HashMap<String, HashSet<String>>>,
}

impl PluginMetaDataRefManager {
    pub fn new() -> Self {
        Self {
            refs: RwLock::new(HashMap::new()),
        }
    }

    /// Register that `ep_key` (e.g., `"EdgionPlugins:default/my-plugin"`) references
    /// `pm_key` (e.g., `"default/my-pm"`).
    pub fn add_ref(&self, pm_key: String, ep_key: String) {
        self.refs.write().entry(pm_key).or_default().insert(ep_key);
    }

    /// Remove all references from a given `ep_key` (call on EdgionPlugins delete or update).
    pub fn clear_refs_from(&self, ep_key: &str) {
        let mut map = self.refs.write();
        for refs in map.values_mut() {
            refs.remove(ep_key);
        }
    }

    /// Get all `ep_key`s that reference a given `pm_key`.
    pub fn get_refs(&self, pm_key: &str) -> Vec<String> {
        self.refs
            .read()
            .get(pm_key)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Clear all refs (for reset).
    pub fn clear_all(&self) {
        self.refs.write().clear();
    }
}

impl Default for PluginMetaDataRefManager {
    fn default() -> Self {
        Self::new()
    }
}
