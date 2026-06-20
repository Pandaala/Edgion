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
use edgion_resources::matcher::ip_radix_tree::IpRadixMatcher;
use edgion_resources::resources::edgion_config_data::{
    ConfigEntry, EdgionConfigData, EdgionConfigDataRef, RegionRouteOverride,
};

static GLOBAL_EDGION_CONFIG_DATA_STORE: LazyLock<Arc<EdgionConfigDataStore>> =
    LazyLock::new(|| Arc::new(EdgionConfigDataStore::new()));

pub fn get_global_edgion_config_data_store() -> Arc<EdgionConfigDataStore> {
    GLOBAL_EDGION_CONFIG_DATA_STORE.clone()
}

/// Flatten optional `EdgionConfigDataRef`s into store keys ("namespace/name"),
/// defaulting an unspecified ref namespace to `parent_namespace`. Pure; called at
/// plugin construction (cold path).
pub fn resolve_ip_list_ref_keys(refs: Option<&Vec<EdgionConfigDataRef>>, parent_namespace: &str) -> Vec<String> {
    refs.map(|v| {
        v.iter()
            .filter(|r| r.ref_denied.is_none())
            .map(|r| format!("{}/{}", r.namespace.as_deref().unwrap_or(parent_namespace), r.name))
            .collect()
    })
    .unwrap_or_default()
}

/// Resolve already-flattened ref keys to their pre-compiled IpList matchers against
/// the global store. Unresolved keys (absent/disabled/uncompilable IpList) are
/// silently skipped — fail-closed direction is enforced by the caller's `allow_mode`.
/// Empty `keys` returns an empty Vec WITHOUT touching the store (hot-path fast path).
/// NOTE: runs on the request/connection hot path — MUST NOT log (data-plane no-tracing).
pub fn resolve_ip_list_matchers(keys: &[String]) -> Vec<Arc<IpRadixMatcher>> {
    if keys.is_empty() {
        return Vec::new();
    }
    let store = get_global_edgion_config_data_store();
    keys.iter()
        .filter_map(|k| store.get_ip_list_matcher_by_key(k))
        .collect()
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

/// Typed index for `IpList` EdgionConfigData resources, keyed "namespace/name".
/// Each `IpList`'s groups are pre-compiled into one `IpRadixMatcher` at conf-change
/// time so the request path never rebuilds. A disabled (`spec.enable=false`) or
/// uncompilable IpList produces NO entry (treated as a missing reference).
type IpListMatcherMap = HashMap<String, Arc<IpRadixMatcher>>;

/// All typed indexes bundled into a single struct so they can be swapped atomically.
///
/// Note: `Selector` (GlobalConnectionIpRestriction's active-profile selector) is NOT
/// indexed here — its consumer reads the raw entry via `get_raw_by_key` and type-guards
/// inline, so a typed index would be unused. Add one only if a typed consumer appears.
struct TypedIndexes {
    region_route_overrides: RegionRouteOverrideMap,
    ip_list: IpListMatcherMap,
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
                    ip_list: HashMap::new(),
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

    /// Look up the pre-compiled `IpRadixMatcher` for an `IpList` by "namespace/name".
    /// Returns `None` if absent, disabled, or uncompilable.
    pub fn get_ip_list_matcher_by_key(&self, key: &str) -> Option<Arc<IpRadixMatcher>> {
        let snap = self.snapshot.load();
        snap.indexes.ip_list.get(key).cloned()
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
        use edgion_resources::matcher::ip_cidr_helpers::build_grouped_matcher_partial;

        let mut rr_overrides = RegionRouteOverrideMap::new();
        let mut ip_list = IpListMatcherMap::new();

        for (key, ed) in raw {
            match &ed.spec.data {
                ConfigEntry::KeyList(_) => {
                    // No typed index — no request-time consumer.
                }
                ConfigEntry::IpList(d) => {
                    // Honor the envelope kill-switch: a disabled IpList resolves as absent.
                    if ed.spec.enable {
                        // Partial-good: always produce a usable matcher; log every skipped entry.
                        // Cold path (conf change), so tracing is allowed here.
                        let (matcher, entry_errors) = build_grouped_matcher_partial(&d.items);
                        for e in &entry_errors {
                            tracing::warn!(
                                component = "edgion_config_data_store",
                                key = %key,
                                entry = %e,
                                "IpList entry skipped (partial-good); valid entries kept",
                            );
                        }
                        ip_list.insert(key.clone(), Arc::new(matcher));
                    }
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
            ip_list,
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
                visibility: Default::default(),
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
                visibility: Default::default(),
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

        // KeyList has no typed index; IpList is now indexed (asserted separately).
        // Raw count and keys still reflect both.
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
                visibility: Default::default(),
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

    #[test]
    fn ip_list_is_indexed_and_matches() {
        use edgion_resources::resources::edgion_config_data::IpListData;
        let store = EdgionConfigDataStore::new();
        let ed = {
            let spec = EdgionConfigDataSpec {
                enable: true,
                active: None,
                visibility: Default::default(),
                data: ConfigEntry::IpList(IpListData {
                    items: vec![IpGroup {
                        name: "office".into(),
                        description: None,
                        cidrs: vec!["10.0.0.0/8".into()],
                    }],
                }),
            };
            let mut ed = EdgionConfigData::new("ip-allow", spec);
            ed.metadata.namespace = Some("ns1".to_string());
            Arc::new(ed)
        };
        let mut raw: RawMap = HashMap::new();
        raw.insert("ns1/ip-allow".to_string(), ed);
        store.replace_all(raw);

        let m = store.get_ip_list_matcher_by_key("ns1/ip-allow").expect("indexed");
        assert_eq!(m.matched_group(&"10.1.2.3".parse().unwrap()), Some("office"));
        assert!(store.get_ip_list_matcher_by_key("ns1/missing").is_none());
    }

    #[test]
    fn ip_list_disabled_is_not_indexed() {
        use edgion_resources::resources::edgion_config_data::IpListData;
        let store = EdgionConfigDataStore::new();
        let ed = {
            let spec = EdgionConfigDataSpec {
                enable: false,
                active: None,
                visibility: Default::default(),
                data: ConfigEntry::IpList(IpListData {
                    items: vec![IpGroup {
                        name: "g".into(),
                        description: None,
                        cidrs: vec!["10.0.0.0/8".into()],
                    }],
                }),
            };
            let mut ed = EdgionConfigData::new("ip-off", spec);
            ed.metadata.namespace = Some("ns2".to_string());
            Arc::new(ed)
        };
        let mut raw: RawMap = HashMap::new();
        raw.insert("ns2/ip-off".to_string(), ed);
        store.replace_all(raw);
        assert!(store.get_ip_list_matcher_by_key("ns2/ip-off").is_none());
    }

    #[test]
    fn ip_list_empty_items_yields_empty_matcher() {
        use edgion_resources::resources::edgion_config_data::IpListData;
        let store = EdgionConfigDataStore::new();
        let ed = {
            let spec = EdgionConfigDataSpec {
                enable: true,
                active: None,
                visibility: Default::default(),
                data: ConfigEntry::IpList(IpListData { items: vec![] }),
            };
            let mut ed = EdgionConfigData::new("ip-empty", spec);
            ed.metadata.namespace = Some("ns3".to_string());
            Arc::new(ed)
        };
        let mut raw: RawMap = HashMap::new();
        raw.insert("ns3/ip-empty".to_string(), ed);
        store.replace_all(raw);
        // Partial-good build (`build_grouped_matcher_partial`): an ENABLED IpList is always
        // indexed, even with no items — it produces a present-but-empty matcher that matches
        // nothing. For a whitelist (allowRefs) consumer this is deny-all (fail-closed); the
        // *disabled* IpList is the one that resolves as absent/`None` (see `ip_list_disabled`).
        let m = store
            .get_ip_list_matcher_by_key("ns3/ip-empty")
            .expect("an enabled IpList is indexed under partial-good, even when empty");
        assert_eq!(m.matched_group(&"10.1.2.3".parse().unwrap()), None);
    }

    #[test]
    fn resolve_ref_keys_defaults_namespace() {
        use edgion_resources::resources::edgion_config_data::EdgionConfigDataRef;
        let refs = vec![
            EdgionConfigDataRef {
                name: "a".into(),
                namespace: None,
                ..Default::default()
            },
            EdgionConfigDataRef {
                name: "b".into(),
                namespace: Some("other".into()),
                ..Default::default()
            },
        ];
        let keys = resolve_ip_list_ref_keys(Some(&refs), "parent");
        assert_eq!(keys, vec!["parent/a".to_string(), "other/b".to_string()]);
        assert!(resolve_ip_list_ref_keys(None, "parent").is_empty());
    }

    #[test]
    fn ref_denied_entries_are_skipped() {
        use edgion_resources::resources::edgion_config_data::EdgionConfigDataRef;
        let refs = vec![
            EdgionConfigDataRef {
                name: "a".into(),
                namespace: None,
                ..Default::default()
            },
            EdgionConfigDataRef {
                name: "b".into(),
                namespace: Some("other".into()),
                ref_denied: Some(edgion_resources::resources::common::RefDenied {
                    target_namespace: "other".into(),
                    target_name: "b".into(),
                    reason: None,
                }),
            },
        ];
        let keys = resolve_ip_list_ref_keys(Some(&refs), "ns");
        assert_eq!(keys, vec!["ns/a".to_string()]); // "other/b" dropped
    }
}
