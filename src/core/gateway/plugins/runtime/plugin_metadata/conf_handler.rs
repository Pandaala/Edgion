//! PluginMetaData gateway store and ConfHandler
//!
//! ArcSwap-backed store with pre-indexed typed maps for zero-clone lookups.
//!
//! At conf change time (low frequency):
//!   ConfHandler receives `Arc<PluginMetaData>` → stores raw + rebuilds typed indexes.
//!   enum `MetaDataEntry` is resolved once here, not per request.
//!
//! At request time (high frequency):
//!   Typed getters return `Arc<Config>` — no cloning, no enum iteration.

use arc_swap::ArcSwap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::LazyLock;

use crate::core::common::conf_sync::traits::ConfHandler;
use crate::types::resources::plugin_metadata::{
    ClusterRegionRouteConfig, GlobalConnectionIpRestrictionData, IpListData, KeyListData, MetaDataEntry,
    PluginMetaData, ServiceRegionRouteConfig,
};

static GLOBAL_PLUGIN_METADATA_STORE: LazyLock<Arc<PluginMetaDataGatewayStore>> =
    LazyLock::new(|| Arc::new(PluginMetaDataGatewayStore::new()));

pub fn get_global_plugin_metadata_store() -> Arc<PluginMetaDataGatewayStore> {
    GLOBAL_PLUGIN_METADATA_STORE.clone()
}

pub fn create_plugin_metadata_handler() -> Box<dyn ConfHandler<PluginMetaData> + Send + Sync> {
    Box::new(get_global_plugin_metadata_store())
}

/// Count PluginMetaData entries in the global store.
pub fn count_plugin_metadata() -> usize {
    GLOBAL_PLUGIN_METADATA_STORE.count()
}

/// List all keys ("namespace/name") in the global PluginMetaData store.
#[allow(dead_code)] // Consumed by validator/sample-diagnostics (Task 15/16); allow until then.
pub fn plugin_metadata_keys() -> Vec<String> {
    GLOBAL_PLUGIN_METADATA_STORE.keys()
}

/// Raw PluginMetaData map. Key: "namespace/name" (e.g. "edgion-default/global-region-conf").
type RawMap = HashMap<String, Arc<PluginMetaData>>;

/// Type-indexed config maps. Key: same "namespace/name" as the parent PluginMetaData.
type ClusterRegionRouteConfigMap = HashMap<String, Arc<ClusterRegionRouteConfig>>;
type ServiceRegionRouteConfigMap = HashMap<String, Arc<ServiceRegionRouteConfig>>;
type KeyListConfigMap = HashMap<String, Arc<KeyListData>>;
type IpListConfigMap = HashMap<String, Arc<IpListData>>;
type GlobalConnectionIpRestrictionMap = HashMap<String, Arc<GlobalConnectionIpRestrictionData>>;

/// All typed indexes bundled into a single struct so they can be swapped atomically.
struct TypedIndexes {
    cluster_region_route_configs: ClusterRegionRouteConfigMap,
    service_region_route_configs: ServiceRegionRouteConfigMap,
    key_list_configs: KeyListConfigMap,
    ip_list_configs: IpListConfigMap,
    global_connection_ip_restrictions: GlobalConnectionIpRestrictionMap,
}

pub struct PluginMetaDataGatewayStore {
    /// Internal raw PluginMetaData storage, keyed by "namespace/name".
    /// Not exposed publicly — used only for incremental updates (`partial_update`),
    /// so that add/remove deltas can be applied before rebuilding typed indexes.
    raw: ArcSwap<RawMap>,

    /// All pre-extracted typed indexes; swapped as one atomic unit on conf change.
    indexes: ArcSwap<TypedIndexes>,
}

impl Default for PluginMetaDataGatewayStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginMetaDataGatewayStore {
    pub fn new() -> Self {
        Self {
            raw: ArcSwap::from_pointee(HashMap::new()),
            indexes: ArcSwap::from_pointee(TypedIndexes {
                cluster_region_route_configs: HashMap::new(),
                service_region_route_configs: HashMap::new(),
                key_list_configs: HashMap::new(),
                ip_list_configs: HashMap::new(),
                global_connection_ip_restrictions: HashMap::new(),
            }),
        }
    }

    // ── Audit / stats ───────────────────────────────────────────────────────

    /// Number of PluginMetaData entries currently in the store.
    pub fn count(&self) -> usize {
        self.raw.load().len()
    }

    /// Keys currently in the store (snapshot copy).
    #[allow(dead_code)] // Consumed by validator/sample-diagnostics (Task 15/16); allow until then.
    pub fn keys(&self) -> Vec<String> {
        self.raw.load().keys().cloned().collect()
    }

    // ── Typed getters (zero-clone, returns Arc) ─────────────────────────────

    pub fn get_cluster_region_route_config(
        &self,
        namespace: &str,
        name: &str,
    ) -> Option<Arc<ClusterRegionRouteConfig>> {
        let indexes = self.indexes.load();
        indexes
            .cluster_region_route_configs
            .get(&format!("{}/{}", namespace, name))
            .cloned()
    }

    pub fn get_service_region_route_config(
        &self,
        namespace: &str,
        name: &str,
    ) -> Option<Arc<ServiceRegionRouteConfig>> {
        let indexes = self.indexes.load();
        indexes
            .service_region_route_configs
            .get(&format!("{}/{}", namespace, name))
            .cloned()
    }

    pub fn get_key_list_config(&self, namespace: &str, name: &str) -> Option<Arc<KeyListData>> {
        let indexes = self.indexes.load();
        indexes
            .key_list_configs
            .get(&format!("{}/{}", namespace, name))
            .cloned()
    }

    pub fn get_ip_list_config(&self, namespace: &str, name: &str) -> Option<Arc<IpListData>> {
        let indexes = self.indexes.load();
        indexes.ip_list_configs.get(&format!("{}/{}", namespace, name)).cloned()
    }

    pub fn get_global_connection_ip_restriction(
        &self,
        namespace: &str,
        name: &str,
    ) -> Option<Arc<GlobalConnectionIpRestrictionData>> {
        let indexes = self.indexes.load();
        indexes
            .global_connection_ip_restrictions
            .get(&format!("{}/{}", namespace, name))
            .cloned()
    }

    pub fn get_global_connection_ip_restriction_by_key(
        &self,
        key: &str,
    ) -> Option<Arc<GlobalConnectionIpRestrictionData>> {
        let indexes = self.indexes.load();
        indexes.global_connection_ip_restrictions.get(key).cloned()
    }

    pub fn get_service_region_route_config_by_key(&self, key: &str) -> Option<Arc<ServiceRegionRouteConfig>> {
        let indexes = self.indexes.load();
        indexes.service_region_route_configs.get(key).cloned()
    }

    pub fn get_cluster_region_route_config_by_key(&self, key: &str) -> Option<Arc<ClusterRegionRouteConfig>> {
        let indexes = self.indexes.load();
        indexes.cluster_region_route_configs.get(key).cloned()
    }

    // ── Index rebuild ───────────────────────────────────────────────────────

    /// Rebuild all typed indexes from the raw map.
    /// Called at conf change time (low frequency).
    fn rebuild_indexes(&self, raw: &RawMap) {
        let mut global_rr = ClusterRegionRouteConfigMap::new();
        let mut service_rr = ServiceRegionRouteConfigMap::new();
        let mut key_lists = KeyListConfigMap::new();
        let mut ip_lists = IpListConfigMap::new();
        let mut global_ip_restrictions = GlobalConnectionIpRestrictionMap::new();

        for (key, pm) in raw {
            match &pm.spec.metadata {
                MetaDataEntry::ClusterRegionRoute(cfg) => {
                    global_rr.insert(key.clone(), Arc::new(cfg.clone()));
                }
                MetaDataEntry::ServiceRegionRoute(cfg) => {
                    service_rr.insert(key.clone(), Arc::new(cfg.clone()));
                }
                MetaDataEntry::KeyList(cfg) => {
                    key_lists.insert(key.clone(), Arc::new(cfg.clone()));
                }
                MetaDataEntry::IpList(cfg) => {
                    ip_lists.insert(key.clone(), Arc::new(cfg.clone()));
                }
                MetaDataEntry::GlobalConnectionIpRestriction(data) => {
                    let mut cloned = data.clone();
                    match cloned.validate_and_init() {
                        Ok(()) => {
                            global_ip_restrictions.insert(key.clone(), Arc::new(cloned));
                        }
                        Err(e) => {
                            tracing::error!(
                                pm_key = %key,
                                error = %e,
                                "GlobalConnectionIpRestriction PM rejected during index rebuild; skipping",
                            );
                        }
                    }
                }
            }
        }

        self.indexes.store(Arc::new(TypedIndexes {
            cluster_region_route_configs: global_rr,
            service_region_route_configs: service_rr,
            key_list_configs: key_lists,
            ip_list_configs: ip_lists,
            global_connection_ip_restrictions: global_ip_restrictions,
        }));
    }

    fn replace_all(&self, data: RawMap) {
        self.rebuild_indexes(&data);
        self.raw.store(Arc::new(data));
    }

    fn update(&self, add_or_update: RawMap, remove: &HashSet<String>) {
        let current = self.raw.load();
        let mut new_map: RawMap = (**current).clone();
        for key in remove {
            new_map.remove(key);
        }
        for (key, value) in add_or_update {
            new_map.insert(key, value);
        }
        self.rebuild_indexes(&new_map);
        self.raw.store(Arc::new(new_map));
    }
}

// ── ConfHandler ─────────────────────────────────────────────────────────────

impl ConfHandler<PluginMetaData> for PluginMetaDataGatewayStore {
    fn full_set(&self, data: &HashMap<String, Arc<PluginMetaData>>) {
        tracing::info!(component = "plugin_metadata_store", cnt = data.len(), "full set");
        self.replace_all(data.clone());
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<PluginMetaData>>,
        update: HashMap<String, Arc<PluginMetaData>>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "plugin_metadata_store",
            add = add.len(),
            update = update.len(),
            rm = remove.len(),
            "partial update"
        );
        let mut add_or_update = add;
        add_or_update.extend(update);
        self.update(add_or_update, &remove);
    }
}

#[cfg(test)]
mod stats_tests {
    use super::*;

    #[test]
    fn count_returns_zero_initially() {
        let store = PluginMetaDataGatewayStore::new();
        assert_eq!(store.count(), 0);
        assert!(store.keys().is_empty());
    }
}
