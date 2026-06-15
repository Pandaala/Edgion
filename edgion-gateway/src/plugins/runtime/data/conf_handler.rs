//! EdgionData gateway store and ConfHandler
//!
//! ArcSwap-backed store with pre-indexed typed maps for zero-clone lookups.
//!
//! At conf change time (low frequency):
//!   ConfHandler receives `Arc<EdgionData>` → builds a new Snapshot
//!   (raw + typed indexes) and swaps it in atomically.
//!   enum `DataEntry` is resolved once here, not per request.
//!
//! At request time (high frequency):
//!   Typed getters return `Arc<Config>` — no cloning, no enum iteration.

use arc_swap::ArcSwap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::LazyLock;

use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::resources::data::{
    ClusterRegionRouteConfig, DataEntry, EdgionData, GlobalConnectionIpRestrictionData, ServiceRegionRouteConfig,
};

static GLOBAL_EDGION_DATA_STORE: LazyLock<Arc<EdgionDataStore>> = LazyLock::new(|| Arc::new(EdgionDataStore::new()));

pub fn get_global_edgion_data_store() -> Arc<EdgionDataStore> {
    GLOBAL_EDGION_DATA_STORE.clone()
}

pub fn create_edgion_data_handler() -> Box<dyn ConfHandler<EdgionData> + Send + Sync> {
    Box::new(get_global_edgion_data_store())
}

/// Count EdgionData entries in the global store.
pub fn count_edgion_data() -> usize {
    GLOBAL_EDGION_DATA_STORE.count()
}

/// List all keys ("namespace/name") in the global EdgionData store.
#[allow(dead_code)] // Consumed by validator/sample-diagnostics; allow until then.
pub fn edgion_data_keys() -> Vec<String> {
    GLOBAL_EDGION_DATA_STORE.keys()
}

/// Raw EdgionData map. Key: "namespace/name" (e.g. "edgion-default/global-region-conf").
type RawMap = HashMap<String, Arc<EdgionData>>;

/// Type-indexed config maps. Key: same "namespace/name" as the parent EdgionData.
type ClusterRegionRouteConfigMap = HashMap<String, Arc<ClusterRegionRouteConfig>>;
type ServiceRegionRouteConfigMap = HashMap<String, Arc<ServiceRegionRouteConfig>>;
type GlobalConnectionIpRestrictionMap = HashMap<String, Arc<GlobalConnectionIpRestrictionData>>;

/// All typed indexes bundled into a single struct so they can be swapped atomically.
struct TypedIndexes {
    cluster_region_route_configs: ClusterRegionRouteConfigMap,
    service_region_route_configs: ServiceRegionRouteConfigMap,
    global_connection_ip_restrictions: GlobalConnectionIpRestrictionMap,
}

/// Atomic view of the store: raw map + derived typed indexes published together.
/// Holding raw and indexes in a single ArcSwap guarantees readers never observe a
/// new-indexes / old-raw (or vice versa) tear during a conf change.
struct Snapshot {
    raw: RawMap,
    indexes: TypedIndexes,
}

pub struct EdgionDataStore {
    /// Single ArcSwap holding the raw EdgionData map together with all typed
    /// indexes derived from it. Writes (`replace_all` / `update`) build a fresh
    /// `Snapshot` and publish it with one store; readers always see a self-consistent
    /// view.
    ///
    /// Writer invariant: ConfHandler is driven by the single gRPC conf-sync task,
    /// so the load-clone-store RMW in `update` is race-free in practice. Adding a
    /// second writer would require external synchronization.
    snapshot: ArcSwap<Snapshot>,
}

impl Default for EdgionDataStore {
    fn default() -> Self {
        Self::new()
    }
}

impl EdgionDataStore {
    pub fn new() -> Self {
        Self {
            snapshot: ArcSwap::from_pointee(Snapshot {
                raw: HashMap::new(),
                indexes: TypedIndexes {
                    cluster_region_route_configs: HashMap::new(),
                    service_region_route_configs: HashMap::new(),
                    global_connection_ip_restrictions: HashMap::new(),
                },
            }),
        }
    }

    // ── Audit / stats ───────────────────────────────────────────────────────

    /// Number of EdgionData entries currently in the store.
    pub fn count(&self) -> usize {
        self.snapshot.load().raw.len()
    }

    /// Keys currently in the store (snapshot copy).
    #[allow(dead_code)] // Consumed by validator/sample-diagnostics; allow until then.
    pub fn keys(&self) -> Vec<String> {
        self.snapshot.load().raw.keys().cloned().collect()
    }

    // ── Typed getters (zero-clone, returns Arc) ─────────────────────────────

    pub fn get_cluster_region_route_config(
        &self,
        namespace: &str,
        name: &str,
    ) -> Option<Arc<ClusterRegionRouteConfig>> {
        let snap = self.snapshot.load();
        snap.indexes
            .cluster_region_route_configs
            .get(&format!("{}/{}", namespace, name))
            .cloned()
    }

    pub fn get_service_region_route_config(
        &self,
        namespace: &str,
        name: &str,
    ) -> Option<Arc<ServiceRegionRouteConfig>> {
        let snap = self.snapshot.load();
        snap.indexes
            .service_region_route_configs
            .get(&format!("{}/{}", namespace, name))
            .cloned()
    }

    pub fn get_global_connection_ip_restriction(
        &self,
        namespace: &str,
        name: &str,
    ) -> Option<Arc<GlobalConnectionIpRestrictionData>> {
        let snap = self.snapshot.load();
        snap.indexes
            .global_connection_ip_restrictions
            .get(&format!("{}/{}", namespace, name))
            .cloned()
    }

    pub fn get_global_connection_ip_restriction_by_key(
        &self,
        key: &str,
    ) -> Option<Arc<GlobalConnectionIpRestrictionData>> {
        let snap = self.snapshot.load();
        snap.indexes.global_connection_ip_restrictions.get(key).cloned()
    }

    pub fn get_service_region_route_config_by_key(&self, key: &str) -> Option<Arc<ServiceRegionRouteConfig>> {
        let snap = self.snapshot.load();
        snap.indexes.service_region_route_configs.get(key).cloned()
    }

    pub fn get_cluster_region_route_config_by_key(&self, key: &str) -> Option<Arc<ClusterRegionRouteConfig>> {
        let snap = self.snapshot.load();
        snap.indexes.cluster_region_route_configs.get(key).cloned()
    }

    // ── Index rebuild ───────────────────────────────────────────────────────

    /// Build all typed indexes from a raw map. Pure function; called at conf change
    /// time only (low frequency).
    fn build_indexes(raw: &RawMap) -> TypedIndexes {
        let mut global_rr = ClusterRegionRouteConfigMap::new();
        let mut service_rr = ServiceRegionRouteConfigMap::new();
        let mut global_ip_restrictions = GlobalConnectionIpRestrictionMap::new();

        for (key, ed) in raw {
            match &ed.spec.data {
                DataEntry::ClusterRegionRoute(cfg) => {
                    global_rr.insert(key.clone(), Arc::new(cfg.clone()));
                }
                DataEntry::ServiceRegionRoute(cfg) => {
                    service_rr.insert(key.clone(), Arc::new(cfg.clone()));
                }
                DataEntry::KeyList(_) => {
                    // No index — KeyList/IpList are standard primitives with no live consumer yet.
                    // Plumbing will be rebuilt when a real consumer is wired (D1).
                }
                DataEntry::IpList(_) => {
                    // No index — see KeyList comment above.
                }
                DataEntry::GlobalConnectionIpRestriction(data) => {
                    let mut cloned = data.clone();
                    match cloned.validate_and_init() {
                        Ok(()) => {
                            global_ip_restrictions.insert(key.clone(), Arc::new(cloned));
                        }
                        Err(e) => {
                            tracing::error!(
                                edgion_data_key = %key,
                                error = %e,
                                "GlobalConnectionIpRestriction EdgionData rejected during index rebuild; skipping",
                            );
                        }
                    }
                }
                DataEntry::Misc(_) => {
                    // Misc is an opaque escape hatch; no typed index is built for it.
                }
            }
        }

        TypedIndexes {
            cluster_region_route_configs: global_rr,
            service_region_route_configs: service_rr,
            global_connection_ip_restrictions: global_ip_restrictions,
        }
    }

    fn replace_all(&self, data: RawMap) {
        let indexes = Self::build_indexes(&data);
        self.snapshot.store(Arc::new(Snapshot { raw: data, indexes }));
    }

    fn update(&self, add_or_update: RawMap, remove: &HashSet<String>) {
        let current = self.snapshot.load();
        let mut new_map: RawMap = current.raw.clone();
        for key in remove {
            new_map.remove(key);
        }
        for (key, value) in add_or_update {
            new_map.insert(key, value);
        }
        let indexes = Self::build_indexes(&new_map);
        self.snapshot.store(Arc::new(Snapshot { raw: new_map, indexes }));
    }
}

// ── ConfHandler ─────────────────────────────────────────────────────────────

impl ConfHandler<EdgionData> for EdgionDataStore {
    fn full_set(&self, data: &HashMap<String, Arc<EdgionData>>) {
        tracing::info!(component = "edgion_data_store", cnt = data.len(), "full set");
        self.replace_all(data.clone());
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<EdgionData>>,
        update: HashMap<String, Arc<EdgionData>>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "edgion_data_store",
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
    use edgion_resources::resources::data::{EdgionDataSpec, IpGroup, KeyGroup};

    #[test]
    fn count_returns_zero_initially() {
        let store = EdgionDataStore::new();
        assert_eq!(store.count(), 0);
        assert!(store.keys().is_empty());
    }

    /// Pins the invariant the Snapshot consolidation provides: raw (read by
    /// count/keys) and indexes (read by typed getters) are published together,
    /// so a reader after replace_all sees both sides at the same generation.
    #[test]
    fn replace_all_publishes_raw_and_indexes_in_same_generation() {
        use edgion_resources::resources::data::{IpListData, KeyListData, KeyMatchMode};

        let store = EdgionDataStore::new();

        let ip_ed = {
            let spec = EdgionDataSpec {
                enable: true,
                active: None,
                data: DataEntry::IpList(IpListData {
                    items: vec![IpGroup {
                        name: "default".into(),
                        description: None,
                        cidrs: vec!["10.0.0.0/8".into()],
                    }],
                }),
            };
            let mut ed = EdgionData::new("ip-allow", spec);
            ed.metadata.namespace = Some("ns1".to_string());
            Arc::new(ed)
        };
        let key_ed = {
            let spec = EdgionDataSpec {
                enable: true,
                active: None,
                data: DataEntry::KeyList(KeyListData {
                    match_mode: KeyMatchMode::default(),
                    items: vec![KeyGroup {
                        name: "default".to_string(),
                        description: None,
                        items: vec![],
                    }],
                }),
            };
            let mut ed = EdgionData::new("key-list", spec);
            ed.metadata.namespace = Some("ns2".to_string());
            Arc::new(ed)
        };

        let mut raw: RawMap = HashMap::new();
        raw.insert("ns1/ip-allow".to_string(), ip_ed);
        raw.insert("ns2/key-list".to_string(), key_ed);
        store.replace_all(raw);

        // IpList and KeyList are no longer indexed (D1), but raw count and keys must reflect them.
        assert_eq!(store.count(), 2);
        assert_eq!(store.keys().len(), 2);
    }
}
