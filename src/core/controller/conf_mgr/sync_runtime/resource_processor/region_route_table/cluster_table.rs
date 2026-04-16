//! Cluster RegionRoute Table
//!
//! Stores ClusterRegionRoute entries indexed by pm_key (namespace/name).
//! Built from PluginMetaData containing ClusterRegionRoute plugin entries.

use parking_lot::RwLock;
use serde::Serialize;
use std::collections::HashMap;

use crate::types::common::KeyGet;
use crate::types::resources::edgion_plugins::plugin_configs::{HashCalcConfig, RegionDef, RouteRule};
use crate::types::resources::plugin_metadata::RouteByKeyConfMatchMeta;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterRegionRouteEntry {
    pub pm_namespace: String,
    pub pm_name: String,
    pub my_region: String,
    pub regions: Vec<RegionDef>,
    pub key_get: Vec<KeyGet>,
    pub hash_key_get: Option<Vec<KeyGet>>,
    pub hash_calc: Option<HashCalcConfig>,
    pub route_rules: Vec<RouteRule>,
    pub route_by_key_conf_match: Option<RouteByKeyConfMatchMeta>,
}

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
