//! Cluster RegionRoute Table
//!
//! Stores ClusterRegionRoute entries indexed by pm_key (namespace/name).
//! Built from PluginMetaData containing ClusterRegionRoute plugin entries.

use parking_lot::RwLock;
use std::collections::HashMap;

pub use crate::types::resources::plugin_metadata::ClusterRegionRouteEntry;

pub struct RegionRouteClusterTable {
    data: RwLock<HashMap<String, ClusterRegionRouteEntry>>,
}

impl RegionRouteClusterTable {
    pub fn new() -> Self {
        Self {
            data: RwLock::new(HashMap::new()),
        }
    }

    pub fn upsert(&self, pm_key: String, entry: ClusterRegionRouteEntry) {
        self.data.write().insert(pm_key, entry);
    }

    pub fn remove(&self, pm_key: &str) {
        self.data.write().remove(pm_key);
    }

    pub fn get(&self, pm_key: &str) -> Option<ClusterRegionRouteEntry> {
        self.data.read().get(pm_key).cloned()
    }

    pub fn all(&self) -> Vec<ClusterRegionRouteEntry> {
        self.data.read().values().cloned().collect()
    }

    pub fn clear(&self) {
        self.data.write().clear();
    }
}
