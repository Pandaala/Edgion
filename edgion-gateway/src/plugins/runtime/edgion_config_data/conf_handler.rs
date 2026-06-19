//! EdgionConfigData gateway store and ConfHandler
//!
//! ArcSwap-backed store with pre-indexed typed maps for zero-clone lookups.
//!
//! At conf change time (low frequency):
//!   ConfHandler receives `Arc<EdgionConfigData>` → builds a new Snapshot
//!   (raw + typed indexes) and swaps it in atomically.
//!   enum `ConfigEntry` is resolved once here, not per request.
//!
//! At request time (high frequency):
//!   Typed getters return `Arc<Config>` — no cloning, no enum iteration.

use arc_swap::ArcSwap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::LazyLock;

use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::resources::edgion_config_data::{ConfigEntry, EdgionConfigData, RegionRouteOverride};

static GLOBAL_EDGION_CONFIG_DATA_STORE: LazyLock<Arc<EdgionConfigDataStore>> =
    LazyLock::new(|| Arc::new(EdgionConfigDataStore::new()));

pub fn get_global_edgion_config_data_store() -> Arc<EdgionConfigDataStore> {
    GLOBAL_EDGION_CONFIG_DATA_STORE.clone()
}

pub fn create_edgion_config_data_handler() -> Box<dyn ConfHandler<EdgionConfigData> + Send + Sync> {
    Box::new(get_global_edgion_config_data_store())
}

/// Count EdgionConfigData entries in the global store.
pub fn count_edgion_config_data() -> usize {
    GLOBAL_EDGION_CONFIG_DATA_STORE.count()
}

/// List all keys ("namespace/name") in the global EdgionConfigData store.
#[allow(dead_code)] // Consumed by validator/sample-diagnostics; allow until then.
pub fn edgion_config_data_keys() -> Vec<String> {
    GLOBAL_EDGION_CONFIG_DATA_STORE.keys()
}

/// Raw EdgionConfigData map. Key: "namespace/name" (e.g. "edgion-default/region-override").
type RawMap = HashMap<String, Arc<EdgionConfigData>>;

/// Typed index for `RegionRouteOverride` overlay EdgionConfigData resources.
/// Key: "namespace/name". Values carry the overlay config; no pre-compilation needed.
type RegionRouteOverrideMap = HashMap<String, Arc<RegionRouteOverride>>;

/// All typed indexes bundled into a single struct so they can be swapped atomically.
///
/// Note: `Selector` (GlobalConnectionIpRestriction's active-profile selector) is NOT
/// indexed here — its consumer reads the raw entry via `get_raw_by_key` and type-guards
/// inline, so a typed index would be unused. Add one only if a typed consumer appears.
struct TypedIndexes {
    region_route_overrides: RegionRouteOverrideMap,
}

/// Atomic view of the store: raw map + derived typed indexes published together.
/// Holding raw and indexes in a single ArcSwap guarantees readers never observe a
/// new-indexes / old-raw (or vice versa) tear during a conf change.
struct Snapshot {
    raw: RawMap,
    indexes: TypedIndexes,
}

pub struct EdgionConfigDataStore {
    /// Single ArcSwap holding the raw EdgionConfigData map together with all typed
    /// indexes derived from it. Writes (`replace_all` / `update`) build a fresh
    /// `Snapshot` and publish it with one store; readers always see a self-consistent
    /// view.
    ///
    /// Writer invariant: ConfHandler is driven by the single gRPC conf-sync task,
    /// so the load-clone-store RMW in `update` is race-free in practice. Adding a
    /// second writer would require external synchronization.
    snapshot: ArcSwap<Snapshot>,
}

impl Default for EdgionConfigDataStore {
    fn default() -> Self {
        Self::new()
    }
}

impl EdgionConfigDataStore {
    pub fn new() -> Self {
        Self {
            snapshot: ArcSwap::from_pointee(Snapshot {
                raw: HashMap::new(),
                indexes: TypedIndexes {
                    region_route_overrides: HashMap::new(),
                },
            }),
        }
    }

    // ── Audit / stats ───────────────────────────────────────────────────────

    /// Number of EdgionConfigData entries currently in the store.
    pub fn count(&self) -> usize {
        self.snapshot.load().raw.len()
    }

    /// Keys currently in the store (snapshot copy).
    #[allow(dead_code)] // Consumed by validator/sample-diagnostics; allow until then.
    pub fn keys(&self) -> Vec<String> {
        self.snapshot.load().raw.keys().cloned().collect()
    }

    // ── Typed getters (zero-clone, returns Arc) ─────────────────────────────

    /// Look up a `RegionRouteOverride` overlay entry by "namespace/name" key.
    pub fn get_region_route_override_by_key(&self, key: &str) -> Option<Arc<RegionRouteOverride>> {
        let snap = self.snapshot.load();
        snap.indexes.region_route_overrides.get(key).cloned()
    }

    /// Look up a raw `EdgionConfigData` entry by "namespace/name" key.
    /// Used by the GCIR plugin to perform the type guard before overlay application.
    pub fn get_raw_by_key(&self, key: &str) -> Option<Arc<EdgionConfigData>> {
        let snap = self.snapshot.load();
        snap.raw.get(key).cloned()
    }

    // ── Index rebuild ───────────────────────────────────────────────────────

    /// Build all typed indexes from a raw map. Pure function; called at conf change
    /// time only (low frequency).
    fn build_indexes(raw: &RawMap) -> TypedIndexes {
        let mut rr_overrides = RegionRouteOverrideMap::new();

        for (key, ed) in raw {
            match &ed.spec.data {
                ConfigEntry::KeyList(_) => {
                    // No typed index — no request-time consumer.
                }
                ConfigEntry::IpList(_) => {
                    // No typed index — no request-time consumer.
                }
                ConfigEntry::Selector(_) => {
                    // Active-profile selector. Not indexed — the GlobalConnectionIpRestriction
                    // plugin reads it via `get_raw_by_key` and type-guards inline.
                }
                ConfigEntry::RegionRouteOverride(overlay) => {
                    // Typed overlay for RegionRoute. Stored as-is.
                    // Gateway plugin reads this at request time and applies field-level replace.
                    rr_overrides.insert(key.clone(), Arc::new(overlay.clone()));
                }
                ConfigEntry::Misc(_) => {
                    // Misc is an opaque escape hatch; no typed index is built for it.
                }
            }
        }

        TypedIndexes {
            region_route_overrides: rr_overrides,
        }
    }

    /// Insert or overwrite a single [`EdgionConfigData`] entry in the store.
    ///
    /// This method is only available in `#[cfg(test)]` builds. It uses an **atomic**
    /// `rcu` (compare-and-swap with retry) rather than a plain load-store, so concurrent
    /// inserts from parallel test threads — including tests in *other* files sharing the
    /// process-global store — accumulate instead of clobbering each other (no lost update).
    /// Tests must still use unique keys to avoid overwriting each other's intended values.
    #[cfg(test)]
    pub fn insert_raw_for_test(&self, key: impl Into<String>, data: Arc<EdgionConfigData>) {
        let key = key.into();
        self.snapshot.rcu(|cur| {
            let mut new_map: RawMap = cur.raw.clone();
            new_map.insert(key.clone(), Arc::clone(&data));
            let indexes = Self::build_indexes(&new_map);
            Arc::new(Snapshot { raw: new_map, indexes })
        });
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

impl ConfHandler<EdgionConfigData> for EdgionConfigDataStore {
    fn full_set(&self, data: &HashMap<String, Arc<EdgionConfigData>>) {
        tracing::info!(component = "edgion_config_data_store", cnt = data.len(), "full set");
        self.replace_all(data.clone());
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<EdgionConfigData>>,
        update: HashMap<String, Arc<EdgionConfigData>>,
        remove: HashSet<String>,
    ) {
        tracing::info!(
            component = "edgion_config_data_store",
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
    use edgion_resources::resources::edgion_config_data::{EdgionConfigDataSpec, IpGroup, KeyGroup};

    #[test]
    fn count_returns_zero_initially() {
        let store = EdgionConfigDataStore::new();
        assert_eq!(store.count(), 0);
        assert!(store.keys().is_empty());
    }

    /// Pins the invariant the Snapshot consolidation provides: raw (read by
    /// count/keys) and indexes (read by typed getters) are published together,
    /// so a reader after replace_all sees both sides at the same generation.
    #[test]
    fn replace_all_publishes_raw_and_indexes_in_same_generation() {
        use edgion_resources::resources::edgion_config_data::{IpListData, KeyListData, KeyMatchMode};

        let store = EdgionConfigDataStore::new();

        let ip_ed = {
            let spec = EdgionConfigDataSpec {
                enable: true,
                active: None,
                data: ConfigEntry::IpList(IpListData {
                    items: vec![IpGroup {
                        name: "default".into(),
                        description: None,
                        cidrs: vec!["10.0.0.0/8".into()],
                    }],
                }),
            };
            let mut ed = EdgionConfigData::new("ip-allow", spec);
            ed.metadata.namespace = Some("ns1".to_string());
            Arc::new(ed)
        };
        let key_ed = {
            let spec = EdgionConfigDataSpec {
                enable: true,
                active: None,
                data: ConfigEntry::KeyList(KeyListData {
                    match_mode: KeyMatchMode::default(),
                    items: vec![KeyGroup {
                        name: "default".to_string(),
                        description: None,
                        items: vec![],
                    }],
                }),
            };
            let mut ed = EdgionConfigData::new("key-list", spec);
            ed.metadata.namespace = Some("ns2".to_string());
            Arc::new(ed)
        };

        let mut raw: RawMap = HashMap::new();
        raw.insert("ns1/ip-allow".to_string(), ip_ed);
        raw.insert("ns2/key-list".to_string(), key_ed);
        store.replace_all(raw);

        // IpList and KeyList have no typed index; raw count and keys still reflect them.
        assert_eq!(store.count(), 2);
        assert_eq!(store.keys().len(), 2);
    }

    #[test]
    fn region_route_override_is_indexed() {
        use edgion_resources::resources::edgion_config_data::RegionRouteOverride;

        let store = EdgionConfigDataStore::new();

        let rr_override_ed = {
            let spec = EdgionConfigDataSpec {
                enable: true,
                active: None,
                data: ConfigEntry::RegionRouteOverride(RegionRouteOverride {
                    enable: None,
                    active: None,
                    regions: None,
                }),
            };
            let mut ed = EdgionConfigData::new("rr-override", spec);
            ed.metadata.namespace = Some("ns-test".to_string());
            Arc::new(ed)
        };

        let mut raw: RawMap = HashMap::new();
        raw.insert("ns-test/rr-override".to_string(), rr_override_ed);
        store.replace_all(raw);

        assert_eq!(store.count(), 1);
        let found = store.get_region_route_override_by_key("ns-test/rr-override");
        assert!(found.is_some(), "RegionRouteOverride must be indexed");
        assert!(found.unwrap().regions.is_none());
    }
}
