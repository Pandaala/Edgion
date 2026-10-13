//! EdgionConfigData Reference Manager
//!
//! Tracks which Gateway/EdgionPlugins/EdgionStreamPlugins resources reference a
//! given EdgionConfigData. Used by EdgionConfigDataHandler to requeue every
//! dependent through its normal workqueue when the target changes.

use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};

/// Tracks resources that reference a given EdgionConfigData.
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

    /// Register that `dependent_key` (for example
    /// `"Gateway:default/my-gateway"`) references `ed_key`.
    pub fn add_ref(&self, ed_key: String, dependent_key: String) {
        self.refs.write().entry(ed_key).or_default().insert(dependent_key);
    }

    /// Remove all references owned by a dependent on update or delete.
    pub fn clear_refs_from(&self, dependent_key: &str) {
        let mut map = self.refs.write();
        map.retain(|_, refs| {
            refs.remove(dependent_key);
            !refs.is_empty()
        });
    }

    /// Get all dependent keys that reference a given `ed_key`.
    pub fn get_refs(&self, ed_key: &str) -> Vec<String> {
        self.refs
            .read()
            .get(ed_key)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Remove the entire forward-direction entry for an EdgionConfigData key.
    ///
    /// This explicitly drops all incoming dependencies. Ordinary target deletion
    /// must retain them so recreation can wake unchanged consumers.
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

#[cfg(test)]
#[path = "../../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/edgion_config_data_utils/ref_manager_test.rs"]
mod tests;
