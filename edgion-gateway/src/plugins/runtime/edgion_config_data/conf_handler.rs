//! EdgionConfigData gateway store and ConfHandler
//!
//! ArcSwap-backed store with pre-indexed typed maps for zero-clone lookups.
//!
//! At conf change time (low frequency):
//!   ConfHandler receives `Arc<EdgionConfigData>` → builds a new Snapshot
//!   (raw + the compiled IpList index) and swaps it in atomically.
//!
//! At request time (high frequency):
//!   IpList consumers load a precompiled matcher. Selector and RegionRoute
//!   consumers load the raw entry and type-guard `ConfigEntry` dynamically.

use arc_swap::ArcSwap;
use smallvec::SmallVec;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::LazyLock;

use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::matcher::ip_radix_tree::IpRadixMatcher;
use edgion_resources::resources::edgion_config_data::{ConfigEntry, EdgionConfigData, EdgionConfigDataRef};

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
/// Only `IpList` is indexed: its live data-plane consumers need a precompiled
/// matcher. `Selector` and `RegionRouteOverride` consumers read and type-guard
/// the raw snapshot. Controller-expanded variants need no Gateway index.
struct TypedIndexes {
    ip_list: IpListMatcherMap,
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
    /// Used by live Selector and RegionRoute consumers to perform their type guard.
    pub fn get_raw_by_key(&self, key: &str) -> Option<Arc<EdgionConfigData>> {
        let snap = self.snapshot.load();
        snap.raw.get(key).cloned()
    }

    // ── Index rebuild ───────────────────────────────────────────────────────

    /// Build all typed indexes from a raw map during a full-set publication.
    fn build_indexes(raw: &RawMap) -> TypedIndexes {
        let mut ip_list = IpListMatcherMap::new();

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
                    // RegionRoute reads and type-guards this variant directly from
                    // the raw snapshot, so no duplicate typed value is needed.
                }
                ConfigEntry::Misc(_) => {
                    // Misc is an opaque escape hatch; no typed index is built for it.
                }
                ConfigEntry::ProxyProtocolTrust(_) => {
                    // Resolved by the Controller into GatewaySpec; the Gateway
                    // never builds a second EdgionConfigData runtime index.
                }
            }
        }

        TypedIndexes { ip_list }
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
            ConfigEntry::KeyList(_)
            | ConfigEntry::RequestAccessUrlAllowList(_)
            | ConfigEntry::Selector(_)
            | ConfigEntry::RegionRouteOverride(_)
            | ConfigEntry::Misc(_)
            | ConfigEntry::ProxyProtocolTrust(_) => {}
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
        };
        let mut changed = false;

        for key in remove {
            changed |= raw.remove(key).is_some();
            indexes.ip_list.remove(key);
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
mod stats_tests {
    use super::*;
    use edgion_resources::resources::edgion_config_data::{
        EdgionConfigDataSpec, IpGroup, IpListData, KeyGroup, RegionRouteOverride, Selector,
    };

    fn ip_list_resource(name: &str, enable: bool, description: Option<&str>, cidr: &str) -> Arc<EdgionConfigData> {
        let spec = EdgionConfigDataSpec {
            enable,
            active: None,
            visibility: Default::default(),
            unknown_fields: Vec::new(),
            data: ConfigEntry::IpList(IpListData {
                items: vec![IpGroup {
                    name: "group".to_string(),
                    description: description.map(str::to_string),
                    cidrs: vec![cidr.to_string()],
                }],
            }),
        };
        let mut resource = EdgionConfigData::new(name, spec);
        resource.metadata.namespace = Some("ns".to_string());
        Arc::new(resource)
    }

    fn region_route_resource(name: &str, active: Option<&str>) -> Arc<EdgionConfigData> {
        let spec = EdgionConfigDataSpec {
            enable: true,
            active: None,
            visibility: Default::default(),
            unknown_fields: Vec::new(),
            data: ConfigEntry::RegionRouteOverride(RegionRouteOverride {
                enable: None,
                active: active.map(str::to_string),
                regions: None,
            }),
        };
        let mut resource = EdgionConfigData::new(name, spec);
        resource.metadata.namespace = Some("ns".to_string());
        Arc::new(resource)
    }

    fn misc_resource(name: &str, value: serde_json::Value) -> Arc<EdgionConfigData> {
        let spec = EdgionConfigDataSpec {
            enable: true,
            active: None,
            visibility: Default::default(),
            unknown_fields: Vec::new(),
            data: ConfigEntry::Misc(value),
        };
        let mut resource = EdgionConfigData::new(name, spec);
        resource.metadata.namespace = Some("ns".to_string());
        Arc::new(resource)
    }

    fn selector_resource(name: &str, active: &str) -> Arc<EdgionConfigData> {
        let spec = EdgionConfigDataSpec {
            enable: true,
            active: None,
            visibility: Default::default(),
            unknown_fields: Vec::new(),
            data: ConfigEntry::Selector(Selector {
                active: Some(active.to_string()),
                description: None,
            }),
        };
        let mut resource = EdgionConfigData::new(name, spec);
        resource.metadata.namespace = Some("ns".to_string());
        Arc::new(resource)
    }

    fn named_ip_list_resource(name: &str, group: &str, cidr: &str) -> Arc<EdgionConfigData> {
        let spec = EdgionConfigDataSpec {
            enable: true,
            active: None,
            visibility: Default::default(),
            unknown_fields: Vec::new(),
            data: ConfigEntry::IpList(IpListData {
                items: vec![IpGroup {
                    name: group.to_string(),
                    description: None,
                    cidrs: vec![cidr.to_string()],
                }],
            }),
        };
        let mut resource = EdgionConfigData::new(name, spec);
        resource.metadata.namespace = Some("ns".to_string());
        Arc::new(resource)
    }

    fn snapshot_matcher<'a>(snapshot: &'a EdgionConfigDataSnapshot, key: &str) -> Option<&'a IpRadixMatcher> {
        let keys = [key.to_string()];
        let mut matchers = SmallVec::new();
        snapshot.append_ip_list_matchers(&keys, &mut matchers);
        matchers.pop()
    }

    #[derive(Clone, Default)]
    struct LogBuffer(Arc<parking_lot::Mutex<Vec<u8>>>);

    impl std::io::Write for LogBuffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn capture_warnings(run: impl FnOnce()) -> String {
        let buffer = LogBuffer::default();
        let writer = buffer.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, run);
        let logs = String::from_utf8(buffer.0.lock().clone()).unwrap();
        logs
    }

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
                unknown_fields: Vec::new(),
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
                unknown_fields: Vec::new(),
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
    fn captured_snapshot_prevents_torn_selector_ip_list_authorization() {
        use edgion_resources::matcher::ip_cidr_helpers::evaluate_ip_access_multi;
        use edgion_resources::resources::edgion_plugins::{AccessDecision, DefaultAction};

        fn selected_deny_matchers<'a>(
            selector_snapshot: &EdgionConfigDataSnapshot,
            matcher_snapshot: &'a EdgionConfigDataSnapshot,
            profile_keys: &HashMap<String, Vec<String>>,
        ) -> SmallVec<[&'a IpRadixMatcher; 4]> {
            let selector = selector_snapshot
                .get_raw_by_key("ns/active")
                .expect("the selector must exist in every complete test generation");
            let ConfigEntry::Selector(selector) = &selector.spec.data else {
                panic!("the active test resource must remain a Selector");
            };
            let active = selector
                .active
                .as_deref()
                .expect("the test selector must name an active profile");
            let keys = profile_keys
                .get(active)
                .expect("every test profile must have configured deny keys");
            let mut matchers = SmallVec::new();
            matcher_snapshot.append_ip_list_matchers(keys, &mut matchers);
            matchers
        }

        let store = EdgionConfigDataStore::new();
        let probe = "203.0.113.7".parse().unwrap();
        let profile_keys = HashMap::from([
            (
                "blue".to_string(),
                vec!["ns/blue-first".to_string(), "ns/blue-second".to_string()],
            ),
            (
                "green".to_string(),
                vec!["ns/green-second".to_string(), "ns/green-first".to_string()],
            ),
        ]);
        store.replace_all(HashMap::from([
            ("ns/active".to_string(), selector_resource("active", "blue")),
            (
                "ns/blue-first".to_string(),
                named_ip_list_resource("blue-first", "blue-first", "203.0.113.7/32"),
            ),
            (
                "ns/blue-second".to_string(),
                named_ip_list_resource("blue-second", "blue-second", "203.0.113.7/32"),
            ),
        ]));
        let old_snapshot = store.capture_snapshot();

        store.update(
            HashMap::from([
                (
                    "ns/green-first".to_string(),
                    named_ip_list_resource("green-first", "green-first", "203.0.113.7/32"),
                ),
                (
                    "ns/green-second".to_string(),
                    named_ip_list_resource("green-second", "green-second", "203.0.113.7/32"),
                ),
            ]),
            HashMap::from([
                ("ns/active".to_string(), selector_resource("active", "green")),
                (
                    "ns/blue-first".to_string(),
                    named_ip_list_resource("blue-first", "blue-first", "198.51.100.1/32"),
                ),
                (
                    "ns/blue-second".to_string(),
                    named_ip_list_resource("blue-second", "blue-second", "198.51.100.2/32"),
                ),
            ]),
            &HashSet::new(),
        );
        let new_snapshot = store.capture_snapshot();

        let torn = selected_deny_matchers(&old_snapshot, &new_snapshot, &profile_keys);
        assert_eq!(torn.len(), 2);
        assert_eq!(torn[0].matched_group(&probe), None);
        assert_eq!(torn[1].matched_group(&probe), None);
        assert!(matches!(
            evaluate_ip_access_multi(&torn, &[], false, &DefaultAction::Allow, &probe),
            AccessDecision::Default(DefaultAction::Allow)
        ));

        let old = selected_deny_matchers(&old_snapshot, &old_snapshot, &profile_keys);
        assert_eq!(old.len(), 2);
        assert_eq!(old[0].matched_group(&probe), Some("blue-first"));
        assert_eq!(old[1].matched_group(&probe), Some("blue-second"));
        assert!(matches!(
            evaluate_ip_access_multi(&old, &[], false, &DefaultAction::Allow, &probe),
            AccessDecision::DenyByGroup(group) if group.as_ref() == "blue-first"
        ));

        let current = selected_deny_matchers(&new_snapshot, &new_snapshot, &profile_keys);
        assert_eq!(current.len(), 2);
        assert_eq!(current[0].matched_group(&probe), Some("green-second"));
        assert_eq!(current[1].matched_group(&probe), Some("green-first"));
        assert!(matches!(
            evaluate_ip_access_multi(&current, &[], false, &DefaultAction::Allow, &probe),
            AccessDecision::DenyByGroup(group) if group.as_ref() == "green-second"
        ));
    }

    #[test]
    fn region_route_override_is_available_from_raw_snapshot() {
        let store = EdgionConfigDataStore::new();

        let rr_override_ed = {
            let spec = EdgionConfigDataSpec {
                enable: true,
                active: None,
                visibility: Default::default(),
                unknown_fields: Vec::new(),
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
        let found = store.get_raw_by_key("ns-test/rr-override").unwrap();
        assert!(matches!(
            &found.spec.data,
            ConfigEntry::RegionRouteOverride(overlay) if overlay.regions.is_none()
        ));
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
                unknown_fields: Vec::new(),
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

        let snapshot = store.capture_snapshot();
        let m = snapshot_matcher(&snapshot, "ns1/ip-allow").expect("indexed");
        assert_eq!(m.matched_group(&"10.1.2.3".parse().unwrap()), Some("office"));
        assert!(snapshot_matcher(&snapshot, "ns1/missing").is_none());
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
                unknown_fields: Vec::new(),
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
        let snapshot = store.capture_snapshot();
        assert!(snapshot_matcher(&snapshot, "ns2/ip-off").is_none());
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
                unknown_fields: Vec::new(),
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
        let snapshot = store.capture_snapshot();
        let m = snapshot_matcher(&snapshot, "ns3/ip-empty")
            .expect("an enabled IpList is indexed under partial-good, even when empty");
        assert_eq!(m.matched_group(&"10.1.2.3".parse().unwrap()), None);
    }

    #[test]
    fn unrelated_update_reuses_unchanged_raw_and_typed_arcs() {
        let store = EdgionConfigDataStore::new();
        let ip = ip_list_resource("ip", true, None, "10.0.0.0/8");
        let route = region_route_resource("route", Some("primary"));
        let misc = misc_resource("misc", serde_json::json!({"generation": 1}));
        store.replace_all(HashMap::from([
            ("ns/ip".to_string(), Arc::clone(&ip)),
            ("ns/route".to_string(), Arc::clone(&route)),
            ("ns/misc".to_string(), misc),
        ]));

        let before_ip_raw = store.get_raw_by_key("ns/ip").unwrap();
        let before_route_raw = store.get_raw_by_key("ns/route").unwrap();
        let before_snapshot = store.capture_snapshot();
        let before_matcher = snapshot_matcher(&before_snapshot, "ns/ip").unwrap();

        store.update(
            HashMap::new(),
            HashMap::from([(
                "ns/misc".to_string(),
                misc_resource("misc", serde_json::json!({"generation": 2})),
            )]),
            &HashSet::new(),
        );

        assert!(Arc::ptr_eq(&before_ip_raw, &store.get_raw_by_key("ns/ip").unwrap()));
        assert!(Arc::ptr_eq(
            &before_route_raw,
            &store.get_raw_by_key("ns/route").unwrap()
        ));
        let after_snapshot = store.capture_snapshot();
        assert!(std::ptr::eq(
            before_matcher,
            snapshot_matcher(&after_snapshot, "ns/ip").unwrap()
        ));
    }

    #[test]
    fn equivalent_update_does_not_publish_a_snapshot() {
        let store = EdgionConfigDataStore::new();
        let original = misc_resource("misc", serde_json::json!({"alpha": 1, "beta": 2}));
        store.replace_all(HashMap::from([("ns/misc".to_string(), Arc::clone(&original))]));
        let before = store.snapshot.load_full();
        let equivalent = Arc::new(original.as_ref().clone());

        store.update(
            HashMap::new(),
            HashMap::from([("ns/misc".to_string(), equivalent)]),
            &HashSet::new(),
        );

        assert!(Arc::ptr_eq(&before, &store.snapshot.load_full()));
        assert!(Arc::ptr_eq(&original, &store.get_raw_by_key("ns/misc").unwrap()));
    }

    #[test]
    fn metadata_and_description_update_raw_but_reuse_ip_matcher() {
        let store = EdgionConfigDataStore::new();
        let original = ip_list_resource("ip", true, Some("old"), "10.0.0.0/8");
        store.replace_all(HashMap::from([("ns/ip".to_string(), Arc::clone(&original))]));
        let before_snapshot = store.snapshot.load_full();
        let before_handle = store.capture_snapshot();
        let before_matcher = snapshot_matcher(&before_handle, "ns/ip").unwrap();
        let mut changed = ip_list_resource("ip", true, Some("new"), "10.0.0.0/8");
        Arc::get_mut(&mut changed)
            .unwrap()
            .metadata
            .labels
            .get_or_insert_default()
            .insert("revision".to_string(), "two".to_string());

        store.update(
            HashMap::new(),
            HashMap::from([("ns/ip".to_string(), Arc::clone(&changed))]),
            &HashSet::new(),
        );

        assert!(!Arc::ptr_eq(&before_snapshot, &store.snapshot.load_full()));
        assert!(Arc::ptr_eq(&changed, &store.get_raw_by_key("ns/ip").unwrap()));
        let after_handle = store.capture_snapshot();
        assert!(std::ptr::eq(
            before_matcher,
            snapshot_matcher(&after_handle, "ns/ip").unwrap()
        ));
    }

    #[test]
    fn ip_list_input_change_and_enable_toggle_replace_or_clear_matcher() {
        let store = EdgionConfigDataStore::new();
        store.replace_all(HashMap::from([(
            "ns/ip".to_string(),
            ip_list_resource("ip", true, None, "10.0.0.0/8"),
        )]));
        let first_snapshot = store.capture_snapshot();
        let first = snapshot_matcher(&first_snapshot, "ns/ip").unwrap();

        store.update(
            HashMap::new(),
            HashMap::from([("ns/ip".to_string(), ip_list_resource("ip", true, None, "192.0.2.0/24"))]),
            &HashSet::new(),
        );
        let second_snapshot = store.capture_snapshot();
        let second = snapshot_matcher(&second_snapshot, "ns/ip").unwrap();
        assert!(!std::ptr::eq(first, second));
        assert_eq!(second.matched_group(&"192.0.2.1".parse().unwrap()), Some("group"));

        store.update(
            HashMap::new(),
            HashMap::from([("ns/ip".to_string(), ip_list_resource("ip", false, None, "192.0.2.0/24"))]),
            &HashSet::new(),
        );
        let disabled_snapshot = store.capture_snapshot();
        assert!(snapshot_matcher(&disabled_snapshot, "ns/ip").is_none());

        store.update(
            HashMap::new(),
            HashMap::from([(
                "ns/ip".to_string(),
                ip_list_resource("ip", true, None, "198.51.100.0/24"),
            )]),
            &HashSet::new(),
        );
        let restored_snapshot = store.capture_snapshot();
        let restored = snapshot_matcher(&restored_snapshot, "ns/ip").unwrap();
        assert_eq!(restored.matched_group(&"198.51.100.2".parse().unwrap()), Some("group"));
    }

    #[test]
    fn variant_transitions_and_removal_leave_no_stale_typed_entry() {
        let store = EdgionConfigDataStore::new();
        store.replace_all(HashMap::from([(
            "ns/shared".to_string(),
            ip_list_resource("shared", true, None, "10.0.0.0/8"),
        )]));

        store.update(
            HashMap::new(),
            HashMap::from([("ns/shared".to_string(), region_route_resource("shared", None))]),
            &HashSet::new(),
        );
        let region_snapshot = store.capture_snapshot();
        assert!(snapshot_matcher(&region_snapshot, "ns/shared").is_none());
        assert!(matches!(
            &store.get_raw_by_key("ns/shared").unwrap().spec.data,
            ConfigEntry::RegionRouteOverride(_)
        ));

        let selector = {
            let mut resource = misc_resource("shared", serde_json::Value::Null);
            Arc::get_mut(&mut resource).unwrap().spec.data = ConfigEntry::Selector(Selector {
                active: Some("green".to_string()),
                description: None,
            });
            resource
        };
        store.update(
            HashMap::new(),
            HashMap::from([("ns/shared".to_string(), selector)]),
            &HashSet::new(),
        );
        let selector_snapshot = store.capture_snapshot();
        assert!(snapshot_matcher(&selector_snapshot, "ns/shared").is_none());
        assert!(matches!(
            &store.get_raw_by_key("ns/shared").unwrap().spec.data,
            ConfigEntry::Selector(_)
        ));

        store.update(
            HashMap::new(),
            HashMap::new(),
            &HashSet::from(["ns/shared".to_string()]),
        );
        assert!(store.get_raw_by_key("ns/shared").is_none());
    }

    #[test]
    fn add_generation_never_reuses_deleted_generation_index() {
        let store = EdgionConfigDataStore::new();
        let initial = ip_list_resource("ip", true, None, "10.0.0.0/8");
        store.replace_all(HashMap::from([("ns/ip".to_string(), Arc::clone(&initial))]));
        let old_snapshot = store.capture_snapshot();
        let old_matcher = snapshot_matcher(&old_snapshot, "ns/ip").unwrap();
        let recreated = Arc::new(initial.as_ref().clone());

        // Cache compression represents delete-then-create as add-only when the
        // final cache contains the key, even though the old generation remains here.
        store.update(
            HashMap::from([("ns/ip".to_string(), Arc::clone(&recreated))]),
            HashMap::new(),
            &HashSet::new(),
        );

        assert!(Arc::ptr_eq(&recreated, &store.get_raw_by_key("ns/ip").unwrap()));
        let recreated_snapshot = store.capture_snapshot();
        assert!(!std::ptr::eq(
            old_matcher,
            snapshot_matcher(&recreated_snapshot, "ns/ip").unwrap()
        ));

        let add_winner = ip_list_resource("ip", true, None, "203.0.113.0/24");
        let ignored_update = ip_list_resource("ip", true, None, "198.51.100.0/24");
        store.update(
            HashMap::from([("ns/ip".to_string(), Arc::clone(&add_winner))]),
            HashMap::from([("ns/ip".to_string(), ignored_update)]),
            &HashSet::from(["ns/ip".to_string()]),
        );
        let winner_snapshot = store.capture_snapshot();
        let matcher = snapshot_matcher(&winner_snapshot, "ns/ip").unwrap();
        assert!(Arc::ptr_eq(&add_winner, &store.get_raw_by_key("ns/ip").unwrap()));
        assert_eq!(matcher.matched_group(&"203.0.113.1".parse().unwrap()), Some("group"));
        assert_eq!(matcher.matched_group(&"198.51.100.1".parse().unwrap()), None);
    }

    #[test]
    fn full_and_partial_publication_produce_equivalent_effective_snapshots() {
        let full = EdgionConfigDataStore::new();
        let partial = EdgionConfigDataStore::new();
        let ip = ip_list_resource("ip", true, None, "192.0.2.0/24");
        let route = region_route_resource("route", Some("secondary"));
        let data = HashMap::from([
            ("ns/ip".to_string(), Arc::clone(&ip)),
            ("ns/route".to_string(), Arc::clone(&route)),
        ]);

        full.replace_all(data.clone());
        partial.update(data, HashMap::new(), &HashSet::new());

        let full_snapshot = full.snapshot.load();
        let partial_snapshot = partial.snapshot.load();
        assert_eq!(full_snapshot.raw.len(), partial_snapshot.raw.len());
        for key in ["ns/ip", "ns/route"] {
            assert!(EdgionConfigDataStore::raw_semantically_equal(
                full_snapshot.raw.get(key).unwrap(),
                partial_snapshot.raw.get(key).unwrap(),
            ));
        }
        drop((full_snapshot, partial_snapshot));
        for store in [&full, &partial] {
            let snapshot = store.capture_snapshot();
            assert_eq!(
                snapshot_matcher(&snapshot, "ns/ip")
                    .unwrap()
                    .matched_group(&"192.0.2.1".parse().unwrap()),
                Some("group")
            );
            let route = store.get_raw_by_key("ns/route").unwrap();
            assert!(matches!(
                &route.spec.data,
                ConfigEntry::RegionRouteOverride(overlay)
                    if overlay.active.as_deref() == Some("secondary")
            ));
        }

        partial.update(
            HashMap::new(),
            HashMap::new(),
            &HashSet::from(["ns/ip".to_string(), "ns/route".to_string()]),
        );
        full.replace_all(HashMap::new());
        assert_eq!(full.count(), 0);
        assert_eq!(partial.count(), 0);
        let partial_snapshot = partial.capture_snapshot();
        assert!(snapshot_matcher(&partial_snapshot, "ns/ip").is_none());
        assert!(partial.get_raw_by_key("ns/route").is_none());
    }

    #[test]
    fn warnings_are_emitted_only_for_recompiled_ip_list() {
        let store = EdgionConfigDataStore::new();
        store.replace_all(HashMap::from([
            (
                "ns/ip".to_string(),
                ip_list_resource("ip", true, Some("old"), "invalid-cidr"),
            ),
            ("ns/misc".to_string(), misc_resource("misc", serde_json::json!(1))),
        ]));

        let unrelated = capture_warnings(|| {
            store.update(
                HashMap::new(),
                HashMap::from([("ns/misc".to_string(), misc_resource("misc", serde_json::json!(2)))]),
                &HashSet::new(),
            );
        });
        assert!(unrelated.is_empty(), "{unrelated}");

        let description_only = capture_warnings(|| {
            store.update(
                HashMap::new(),
                HashMap::from([(
                    "ns/ip".to_string(),
                    ip_list_resource("ip", true, Some("new"), "invalid-cidr"),
                )]),
                &HashSet::new(),
            );
        });
        assert!(description_only.is_empty(), "{description_only}");

        let recompiled = capture_warnings(|| {
            store.update(
                HashMap::new(),
                HashMap::from([(
                    "ns/ip".to_string(),
                    ip_list_resource("ip", true, Some("new"), "still-invalid"),
                )]),
                &HashSet::new(),
            );
        });
        assert_eq!(recompiled.lines().count(), 1, "{recompiled}");
        assert!(recompiled.contains("key=ns/ip"), "{recompiled}");
    }

    #[test]
    fn test_insert_rcu_preserves_concurrent_unique_keys() {
        const WRITERS: usize = 8;
        let store = Arc::new(EdgionConfigDataStore::new());
        let barrier = Arc::new(std::sync::Barrier::new(WRITERS));
        let mut handles = Vec::new();
        for writer in 0..WRITERS {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                let key = format!("ns/concurrent-{writer}");
                store.insert_raw_for_test(
                    &key,
                    misc_resource(&format!("concurrent-{writer}"), serde_json::json!(writer)),
                );
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(store.count(), WRITERS);
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
