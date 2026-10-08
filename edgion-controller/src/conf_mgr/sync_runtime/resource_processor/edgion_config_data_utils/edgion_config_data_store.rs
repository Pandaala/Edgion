//! EdgionConfigData controller store
//!
//! Provides a store for EdgionConfigData resources accessible during EdgionPlugins processing.
//!
//! This is control-plane code with low read/write frequency,
//! so a simple RwLock<HashMap> is sufficient.

use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};

use edgion_resources::prelude_resources::EdgionConfigData;
use edgion_resources::resources::edgion_config_data::ConfigEntry;
use edgion_resources::resources::edgion_config_data::PreparedWafPolicySelections;

pub struct WafPolicySnapshot {
    pub policy: Option<EdgionConfigData>,
    pub children: HashMap<String, Option<EdgionConfigData>>,
    pub selections: Result<Option<PreparedWafPolicySelections>, String>,
}

/// Store for EdgionConfigData resources (controller-side, low-frequency access).
///
/// Key format: "namespace/name" (namespaced resources only).
pub struct EdgionConfigDataStore {
    data: RwLock<HashMap<String, EdgionConfigData>>,
}

impl EdgionConfigDataStore {
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }

    /// Get an EdgionConfigData by namespace and name.
    pub fn get(&self, namespace: Option<&str>, name: &str) -> Option<EdgionConfigData> {
        let key = Self::make_key(namespace, name);
        let map = self.data.read();
        map.get(&key).cloned()
    }

    /// Capture a WAF policy, its bundles, and the consumer's optional Selector
    /// under one read guard. Compilation runs after the guard drops.
    pub fn get_waf_policy_snapshot(&self, policy_key: &str, selector_key: Option<&str>) -> WafPolicySnapshot {
        let map = self.data.read();
        let policy_source = map.get(policy_key);
        let mut children = HashMap::new();
        let mut selections = Ok(None);
        if let Some(ed) = policy_source {
            if let ConfigEntry::WafPolicy(data) = &ed.spec.data {
                selections = data.prepare_selections().map(Some);
                if selections.is_err() {
                    return WafPolicySnapshot {
                        policy: policy_source.cloned(),
                        children,
                        selections,
                    };
                }
                let namespace = ed.metadata.namespace.as_deref().unwrap_or("default");
                let references = data
                    .profiles
                    .values()
                    .flat_map(|profile| profile.bundle_refs.iter().map(|reference| &reference.target));
                for reference in references {
                    let target_namespace = reference.namespace.as_deref().unwrap_or(namespace);
                    let key = format!("{target_namespace}/{}", reference.name);
                    children.entry(key.clone()).or_insert_with(|| map.get(&key).cloned());
                }
            }
        }
        if let Some(key) = selector_key {
            children.entry(key.to_string()).or_insert_with(|| map.get(key).cloned());
        }
        WafPolicySnapshot {
            policy: policy_source.cloned(),
            children,
            selections,
        }
    }

    /// Replace all entries (used during reset).
    pub fn replace_all(&self, data: HashMap<String, EdgionConfigData>) {
        let count = data.len();
        let mut map = self.data.write();
        *map = data;
        tracing::debug!(
            component = "edgion_config_data_store",
            count = count,
            "Replaced all EdgionConfigData entries"
        );
    }

    /// Upsert/remove entries.
    ///
    /// - `upsert`: entries to add or update
    /// - `remove`: keys of entries to remove
    pub fn update(&self, upsert: HashMap<String, EdgionConfigData>, remove: &HashSet<String>) {
        let mut map = self.data.write();
        for key in remove {
            map.remove(key);
        }
        for (key, ed) in upsert {
            map.insert(key, ed);
        }
        tracing::debug!(component = "edgion_config_data_store", "Updated EdgionConfigData store");
    }

    /// Build a "namespace/name" key.
    pub fn make_key(namespace: Option<&str>, name: &str) -> String {
        match namespace {
            Some(ns) => format!("{}/{}", ns, name),
            None => name.to_string(),
        }
    }
}

impl Default for EdgionConfigDataStore {
    fn default() -> Self {
        Self::new()
    }
}
