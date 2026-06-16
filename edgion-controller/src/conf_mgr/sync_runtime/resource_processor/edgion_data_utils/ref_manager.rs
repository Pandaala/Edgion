//! EdgionData Reference Manager
//!
//! Tracks which EdgionPlugins/EdgionStreamPlugins resources reference a given EdgionData.
//! Used by EdgionDataHandler to know which resources to requeue when an EdgionData changes.

use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};

/// Tracks which EdgionPlugins/EdgionStreamPlugins resources reference a given EdgionData.
///
/// Key: `"namespace/ed_name"` -> Set of `"Kind:namespace/ep_name"` referencing it.
pub struct EdgionDataRefManager {
    refs: RwLock<HashMap<String, HashSet<String>>>,
}

impl EdgionDataRefManager {
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

    /// Remove the entire forward-direction entry for a PM key.
    ///
    /// Call this when the PM itself is deleted so stale ep_keys do not linger
    /// and get spuriously requeued if the PM key is later reused.
    pub fn remove_edgion_data(&self, ed_key: &str) {
        self.refs.write().remove(ed_key);
    }

    /// Clear all refs (for reset).
    pub fn clear_all(&self) {
        self.refs.write().clear();
    }
}

impl Default for EdgionDataRefManager {
    fn default() -> Self {
        Self::new()
    }
}
