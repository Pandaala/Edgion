//! EdgionConfigData Reference Manager
//!
//! Tracks which EdgionPlugins/EdgionStreamPlugins resources reference a given EdgionConfigData.
//! Used by EdgionConfigDataHandler to know which resources to requeue when an EdgionConfigData changes.

use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};

/// Tracks which EdgionPlugins/EdgionStreamPlugins resources reference a given EdgionConfigData.
///
/// Key: `"namespace/ed_name"` -> Set of `"Kind:namespace/ep_name"` referencing it.
pub struct EdgionConfigDataRefManager {
    refs: RwLock<HashMap<String, HashSet<String>>>,
}

impl EdgionConfigDataRefManager {
    pub fn new() -> Self {
        Self {
            refs: RwLock::new(HashMap::new()),
        }
    }

    /// Register that `ep_key` (e.g., `"EdgionPlugins:default/my-plugin"`) references
    /// `ed_key` (e.g., `"default/my-pm"`).
    pub fn add_ref(&self, ed_key: String, ep_key: String) {
        self.refs.write().entry(ed_key).or_default().insert(ep_key);
    }

    /// Remove all references from a given `ep_key` (call on EdgionPlugins delete or update).
    pub fn clear_refs_from(&self, ep_key: &str) {
        let mut map = self.refs.write();
        for refs in map.values_mut() {
            refs.remove(ep_key);
        }
    }

    /// Get all `ep_key`s that reference a given `ed_key`.
    pub fn get_refs(&self, ed_key: &str) -> Vec<String> {
        self.refs
            .read()
            .get(ed_key)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Remove the entire forward-direction entry for an EdgionConfigData key.
    ///
    /// Call this when the EdgionConfigData itself is deleted so stale ep_keys
    /// do not linger and get spuriously requeued if the key is later reused.
    pub fn remove_edgion_config_data(&self, ed_key: &str) {
        self.refs.write().remove(ed_key);
    }

    /// Clear all refs (for reset).
    pub fn clear_all(&self) {
        self.refs.write().clear();
    }
}

impl Default for EdgionConfigDataRefManager {
    fn default() -> Self {
        Self::new()
    }
}
