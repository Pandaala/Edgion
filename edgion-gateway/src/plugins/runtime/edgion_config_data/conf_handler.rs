//! EdgionConfigData gateway store and ConfHandler
//!
//! ArcSwap-backed store with pre-indexed typed maps for zero-clone lookups.
//!
//! At conf change time (low frequency):
//!   ConfHandler receives `Arc<EdgionConfigData>` → builds a new Snapshot
//!   (raw + the compiled IpList index + region-table proofs) and swaps it atomically.
//!
//! At request time (high frequency):
//!   IpList consumers load a precompiled matcher. Selector and RegionRoute
//!   consumers load the raw entry and type-guard `ConfigEntry` dynamically;
//!   RegionRoute reads its matching validation proof in that same snapshot.

use arc_swap::ArcSwap;
use smallvec::SmallVec;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::LazyLock;

use edgion_algo::matcher::radix_ip::IpRadixMatcher;
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::resources::edgion_config_data::{
    ConfigEntry, EdgionConfigData, EdgionConfigDataRef, RegionTableValidation,
};

static GLOBAL_EDGION_CONFIG_DATA_STORE: LazyLock<Arc<EdgionConfigDataStore>> =
    LazyLock::new(|| Arc::new(EdgionConfigDataStore::new()));

pub fn get_global_edgion_config_data_store() -> Arc<EdgionConfigDataStore> {
    GLOBAL_EDGION_CONFIG_DATA_STORE.clone()
}

pub(crate) fn capture_global_edgion_config_data_snapshot() -> EdgionConfigDataSnapshot {
    GLOBAL_EDGION_CONFIG_DATA_STORE.capture_snapshot()
}

/// Capture the current generation only when an authorization decision has live
/// IpList references. Inline-only decisions retain their zero-store-load path.
pub(crate) fn capture_global_edgion_config_data_snapshot_for_refs(
    deny_keys: &[String],
    allow_keys: &[String],
) -> Option<EdgionConfigDataSnapshot> {
    if deny_keys.is_empty() && allow_keys.is_empty() {
        None
    } else {
        Some(capture_global_edgion_config_data_snapshot())
    }
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

pub fn create_edgion_config_data_handler() -> Box<dyn ConfHandler<EdgionConfigData> + Send + Sync> {
    Box::new(get_global_edgion_config_data_store())
}

/// Count EdgionConfigData entries in the global store.
pub fn count_edgion_config_data() -> usize {
    GLOBAL_EDGION_CONFIG_DATA_STORE.count()
}

/// Raw EdgionConfigData map. Key: "namespace/name" (e.g. "edgion-default/region-override").
type RawMap = HashMap<String, Arc<EdgionConfigData>>;

/// Typed index for `IpList` EdgionConfigData resources, keyed "namespace/name".
/// Each `IpList`'s groups are pre-compiled into one `IpRadixMatcher` at conf-change
/// time so the request path never rebuilds. A disabled (`spec.enable=false`) or
/// uncompilable IpList produces NO entry (treated as a missing reference).
type IpListMatcherMap = HashMap<String, Arc<IpRadixMatcher>>;

/// Typed indexes bundled with the raw map so they can be swapped atomically.
///
/// `IpList` needs a precompiled matcher. RegionRoute needs only a compact proof,
/// not a duplicate table. Selector uses the raw snapshot. Controller-expanded
/// variants need no Gateway index.
struct TypedIndexes {
    ip_list: IpListMatcherMap,
    /// Validation proof only; region data stays in the matching raw resource.
    region_tables: HashMap<String, RegionTableValidation>,
}

/// Atomic view of the store: raw map + derived typed indexes published together.
/// Holding raw and indexes in a single ArcSwap guarantees readers never observe a
/// new-indexes / old-raw (or vice versa) tear during a conf change.
struct Snapshot {
    raw: RawMap,
    indexes: TypedIndexes,
}

/// One owned, immutable EdgionConfigData generation for a complete authorization
/// decision. Raw Selector reads and every referenced matcher borrow must come from
/// this handle so a concurrent publication cannot tear the decision across epochs.
pub(crate) struct EdgionConfigDataSnapshot {
    inner: Arc<Snapshot>,
}

impl EdgionConfigDataSnapshot {
    pub(crate) fn get_raw_by_key(&self, key: &str) -> Option<&EdgionConfigData> {
        self.inner.raw.get(key).map(Arc::as_ref)
    }

    /// Append matchers in configured key order. Missing, disabled, and invalid
    /// references contribute nothing; the caller retains whitelist intent.
    pub(crate) fn append_ip_list_matchers<'a>(
        &'a self,
        keys: &[String],
        target: &mut SmallVec<[&'a IpRadixMatcher; 4]>,
    ) {
        target.extend(
            keys.iter()
                .filter_map(|key| self.inner.indexes.ip_list.get(key).map(Arc::as_ref)),
        );
    }
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
                    ip_list: HashMap::new(),
                    region_tables: HashMap::new(),
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

    /// Capture one immutable raw-plus-index generation. Authorization consumers
    /// retain this handle until every live reference in the decision resolves.
    pub(crate) fn capture_snapshot(&self) -> EdgionConfigDataSnapshot {
        EdgionConfigDataSnapshot {
            inner: self.snapshot.load_full(),
        }
    }

    /// Look up a raw `EdgionConfigData` entry by "namespace/name" key.
    /// Used by live Selector consumers; RegionRoute uses the paired proof getter.
    pub fn get_raw_by_key(&self, key: &str) -> Option<Arc<EdgionConfigData>> {
        let snap = self.snapshot.load();
        snap.raw.get(key).cloned()
    }

    /// Read an overlay and its validation proof from exactly one generation.
    /// A missing proof for a present `regions` table means that table is invalid.
    pub fn get_region_route_by_key(&self, key: &str) -> Option<(Arc<EdgionConfigData>, Option<RegionTableValidation>)> {
        let snap = self.snapshot.load();
        snap.raw
            .get(key)
            .map(|raw| (Arc::clone(raw), snap.indexes.region_tables.get(key).copied()))
    }

    fn validate_region_table(data: &EdgionConfigData) -> Option<RegionTableValidation> {
        if !data.spec.enable {
            return None;
        }
        let ConfigEntry::RegionRouteOverride(overlay) = &data.spec.data else {
            return None;
        };
        RegionTableValidation::new(overlay.regions.as_deref()?).ok()
    }

    // ── Index rebuild ───────────────────────────────────────────────────────

    /// Build all typed indexes from a raw map during a full-set publication.
    fn build_indexes(raw: &RawMap) -> TypedIndexes {
        let mut ip_list = IpListMatcherMap::new();
        let mut region_tables = HashMap::new();

        for (key, ed) in raw {
            match &ed.spec.data {
                ConfigEntry::KeyList(_) => {
                    // No typed index — no request-time consumer.
                }
                ConfigEntry::IpList(_) => {
                    if let Some(matcher) = Self::compile_ip_list(key, ed.as_ref()) {
                        ip_list.insert(key.clone(), matcher);
                    }
                }
                ConfigEntry::RequestAccessUrlAllowList(_) => {
                    // RequestAccessPolicy URL data is expanded by the Controller.
                    // No Gateway request-time index is built for this variant.
                }
                ConfigEntry::Selector(_) => {
                    // Active-profile selector. Not indexed — the GlobalConnectionIpRestriction
                    // plugin reads it via `get_raw_by_key` and type-guards inline.
                }
                ConfigEntry::RegionRouteOverride(_) => {
                    if let Some(proof) = Self::validate_region_table(ed) {
                        region_tables.insert(key.clone(), proof);
                    }
                }
                ConfigEntry::Misc(_) => {
                    // Misc is an opaque escape hatch; no typed index is built for it.
                }
                ConfigEntry::ProxyProtocolTrust(_) => {
                    // Resolved by the Controller into GatewaySpec; the Gateway
                    // never builds a second EdgionConfigData runtime index.
                }
                ConfigEntry::WafRuleBundle(_) => {
                    // The Controller embeds the pinned source in EdgionPlugins;
                    // the Gateway compiles it with the complete WAF generation.
                }
            }
        }

        TypedIndexes { ip_list, region_tables }
    }

    /// Compare the wire-visible value of two resources. Object field ordering is
    /// intentionally ignored; a serialization failure conservatively means changed.
    fn raw_semantically_equal(left: &Arc<EdgionConfigData>, right: &Arc<EdgionConfigData>) -> bool {
        if Arc::ptr_eq(left, right) {
            return true;
        }
        match (
            serde_json::to_value(left.as_ref()),
            serde_json::to_value(right.as_ref()),
        ) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        }
    }

    /// Whether two IpList resources have identical matcher inputs. Descriptions
    /// and envelope fields other than `enable` are not consumed by the compiler.
    fn ip_list_compiled_inputs_equal(left: &EdgionConfigData, right: &EdgionConfigData) -> bool {
        let (ConfigEntry::IpList(left_data), ConfigEntry::IpList(right_data)) = (&left.spec.data, &right.spec.data)
        else {
            return false;
        };
        left.spec.enable == right.spec.enable
            && left_data.items.len() == right_data.items.len()
            && left_data
                .items
                .iter()
                .zip(&right_data.items)
                .all(|(left, right)| left.name == right.name && left.cidrs == right.cidrs)
    }

    fn compile_ip_list(key: &str, ed: &EdgionConfigData) -> Option<Arc<IpRadixMatcher>> {
        use edgion_resources::matcher::ip_cidr_helpers::build_grouped_matcher_partial;

        let ConfigEntry::IpList(data) = &ed.spec.data else {
            return None;
        };
        if !ed.spec.enable {
            return None;
        }
        let (matcher, entry_errors) = build_grouped_matcher_partial(&data.items);
        for error in &entry_errors {
            tracing::warn!(
                component = "edgion_config_data_store",
                key = %key,
                entry = %error,
                "IpList entry skipped (partial-good); valid entries kept",
            );
        }
        Some(Arc::new(matcher))
    }

    fn apply_changed_entry(
        raw: &mut RawMap,
        indexes: &mut TypedIndexes,
        key: String,
        value: Arc<EdgionConfigData>,
        allow_typed_reuse: bool,
    ) {
        let previous = raw.insert(key.clone(), Arc::clone(&value));
        let previous_ip_list = indexes.ip_list.remove(&key);
        indexes.region_tables.remove(&key);

        match &value.spec.data {
            ConfigEntry::IpList(_) => {
                let reusable = allow_typed_reuse
                    && previous
                        .as_ref()
                        .is_some_and(|previous| Self::ip_list_compiled_inputs_equal(previous.as_ref(), value.as_ref()));
                if reusable {
                    if let Some(matcher) = previous_ip_list {
                        indexes.ip_list.insert(key, matcher);
                        return;
                    }
                }
                if let Some(matcher) = Self::compile_ip_list(&key, value.as_ref()) {
                    indexes.ip_list.insert(key, matcher);
                }
            }
            ConfigEntry::RegionRouteOverride(_) => {
                if let Some(proof) = Self::validate_region_table(&value) {
                    indexes.region_tables.insert(key, proof);
                }
            }
            ConfigEntry::KeyList(_)
            | ConfigEntry::RequestAccessUrlAllowList(_)
            | ConfigEntry::Selector(_)
            | ConfigEntry::Misc(_)
            | ConfigEntry::ProxyProtocolTrust(_)
            | ConfigEntry::WafRuleBundle(_) => {}
        }
    }

    /// Apply one compressed change batch while preserving add/update generation
    /// semantics. Returns `None` when the batch has no wire-visible effect.
    fn build_incremental_snapshot(
        current: &Snapshot,
        add: &RawMap,
        update: &RawMap,
        remove: &HashSet<String>,
    ) -> Option<Arc<Snapshot>> {
        let mut raw = current.raw.clone();
        let mut indexes = TypedIndexes {
            ip_list: current.indexes.ip_list.clone(),
            region_tables: current.indexes.region_tables.clone(),
        };
        let mut changed = false;

        for key in remove {
            changed |= raw.remove(key).is_some();
            indexes.ip_list.remove(key);
            indexes.region_tables.remove(key);
        }

        // Add wins over update for a defensive conflicting batch. A compressed
        // delete-then-create is represented as add-only and must never reuse the
        // deleted generation's typed value, even when its content is identical.
        for (key, value) in update {
            if add.contains_key(key) {
                continue;
            }
            if raw
                .get(key)
                .is_some_and(|previous| Self::raw_semantically_equal(previous, value))
            {
                continue;
            }
            Self::apply_changed_entry(&mut raw, &mut indexes, key.clone(), Arc::clone(value), true);
            changed = true;
        }
        for (key, value) in add {
            Self::apply_changed_entry(&mut raw, &mut indexes, key.clone(), Arc::clone(value), false);
            changed = true;
        }

        changed.then(|| Arc::new(Snapshot { raw, indexes }))
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
            let update = [(key.clone(), Arc::clone(&data))].into_iter().collect();
            Self::build_incremental_snapshot(cur, &HashMap::new(), &update, &HashSet::new())
                .unwrap_or_else(|| Arc::clone(cur))
        });
    }

    fn replace_all(&self, data: RawMap) {
        let indexes = Self::build_indexes(&data);
        self.snapshot.store(Arc::new(Snapshot { raw: data, indexes }));
    }

    fn update(&self, add: RawMap, update: RawMap, remove: &HashSet<String>) {
        let current = self.snapshot.load();
        if let Some(next) = Self::build_incremental_snapshot(&current, &add, &update, remove) {
            self.snapshot.store(next);
        }
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
        self.update(add, update, &remove);
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/runtime/edgion_config_data/conf_handler_test/stats_tests.rs"]
mod stats_tests;
