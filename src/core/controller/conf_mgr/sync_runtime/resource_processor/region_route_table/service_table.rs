//! Service RegionRoute Table
//!
//! Stores ServiceRegionRoute entries indexed by pm_key (namespace/name).
//! Built from PluginMetaData containing ServiceRegionRoute plugin entries.
//! `ref_plugins` and `cluster_pm_ref` are updated by EdgionPlugins handlers.

use parking_lot::RwLock;
use std::collections::HashMap;

pub use crate::types::resources::plugin_metadata::{RefPluginEntry, ServiceRegionRouteEntry};

pub struct RegionRouteServiceTable {
    data: RwLock<HashMap<String, ServiceRegionRouteEntry>>,
}

impl RegionRouteServiceTable {
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }

    /// Update PluginMetaData-sourced fields (including cluster_pm_ref) without touching ref_plugins.
    /// If the entry already exists, preserves ref_plugins.
    pub fn upsert_from_pm(&self, pm_key: String, entry: ServiceRegionRouteEntry) {
        let mut data = self.data.write();
        if let Some(existing) = data.get_mut(&pm_key) {
            existing.pm_namespace = entry.pm_namespace;
            existing.pm_name = entry.pm_name;
            existing.cluster_pm_ref = entry.cluster_pm_ref;
            existing.regions = entry.regions;
        } else {
            data.insert(pm_key, entry);
        }
    }

    pub fn remove(&self, pm_key: &str) {
        self.data.write().remove(pm_key);
    }

    pub fn get(&self, pm_key: &str) -> Option<ServiceRegionRouteEntry> {
        self.data.read().get(pm_key).cloned()
    }

    pub fn all(&self) -> Vec<ServiceRegionRouteEntry> {
        self.data.read().values().cloned().collect()
    }

    /// Add a ref_plugin entry. Creates a stub entry if the PluginMetaData hasn't been processed yet.
    pub fn add_ref_plugin(&self, pm_key: &str, ref_entry: RefPluginEntry) {
        let mut data = self.data.write();
        if let Some(existing) = data.get_mut(pm_key) {
            if !existing.ref_plugins.contains(&ref_entry) {
                existing.ref_plugins.push(ref_entry);
            }
        } else {
            // PluginMetaData hasn't been processed yet — create a stub.
            data.insert(
                pm_key.to_string(),
                ServiceRegionRouteEntry {
                    pm_namespace: String::new(),
                    pm_name: String::new(),
                    cluster_pm_ref: None,
                    regions: vec![],
                    ref_plugins: vec![ref_entry],
                },
            );
        }
    }

    /// Remove all ref_plugin entries for a given resource.
    pub fn clear_ref_plugins_from(&self, kind: &str, namespace: &str, name: &str) {
        let mut data = self.data.write();
        for entry in data.values_mut() {
            entry
                .ref_plugins
                .retain(|r| !(r.kind == kind && r.namespace == namespace && r.name == name));
        }
    }

    pub fn clear(&self) {
        self.data.write().clear();
    }
}
