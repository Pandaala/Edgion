//! Fleet-wide registry of `unknown_fields` findings for `EdgionPlugins` /
//! `EdgionStreamPlugins`, read live by `GET /api/v1/diagnostics/unknown-fields`
//! (`api/diagnostics_handlers.rs`).
//!
//! `unknown_fields` is `#[serde(skip)]` on the resource specs (deliberately —
//! see `tasks/todo/plugin-config-unknown-field-detection/05-issues.md` R-3),
//! so it is invisible through `AdminState::center_list_resources`, which
//! re-serializes the typed object to JSON. This module reads the typed
//! `RawStore` directly instead, mirroring
//! `observe::status_metrics::register_status_renderer` closely enough that
//! the two are registered side by side:
//!
//! - Same three call sites (`conf_center/{file_system,etcd,kubernetes}/resource_controller.rs`),
//!   right next to the existing `register_status_renderer` call, each already
//!   holding the `Arc<dyn RawStore<K>>` that call needs.
//! - Same re-registration contract: keyed by `kind`, so a conf_center reload
//!   (`FileSystemController` / `EtcdController` / `KubernetesController`
//!   reconstruction) REPLACES the prior closure rather than accumulating a
//!   duplicate — see `status_metrics.rs:103` for the fuller rationale, pinned
//!   here by `re_registering_same_kind_replaces_old_source` below.
//!
//! Unlike `status_metrics`, there is no background snapshot loop: this is a
//! live, on-demand query (`collect_unknown_fields`), read directly by the
//! diagnostics handler on each request. The fleet is expected to be small
//! enough, and the call infrequent enough (an operator or CI asking a
//! point-in-time question), that a full live scan is cheap and avoids a
//! second background task plus its own staleness window.
//!
//! `K` is generic at the three call sites, so the concrete `EdgionPlugins` /
//! `EdgionStreamPlugins` type is reached via `Any` downcast — the same idiom
//! `conf_mgr::conf_center::common::populate_unknown_fields_non_fatal` uses at
//! the analogous generic seam (FileSystem/etcd loader).

use parking_lot::Mutex;
use serde::Serialize;
use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use crate::conf_mgr::sync_runtime::resource_processor::raw_store::RawStore;

/// One resource's unknown-field findings, ready for the diagnostics response.
#[derive(Debug, Clone, Serialize)]
pub struct UnknownFieldsEntry {
    pub kind: &'static str,
    pub namespace: String,
    pub name: String,
    pub paths: Vec<String>,
}

/// Per-kind findings source closure signature: scans one kind's store and
/// pushes an entry for every resource carrying at least one finding.
type Source = Box<dyn Fn(&mut Vec<UnknownFieldsEntry>) + Send + Sync>;

/// Registered per-kind sources, keyed by kind name so a second registration
/// for the same kind replaces (never appends to) the first.
fn sources_slot() -> &'static Mutex<HashMap<&'static str, Source>> {
    static SOURCES: OnceLock<Mutex<HashMap<&'static str, Source>>> = OnceLock::new();
    SOURCES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Test-only module-level mutex, mirroring `status_metrics::TEST_MUTEX`:
/// serializes tests that mutate the global `sources_slot()` against parallel
/// `cargo test` threads in the same process.
#[cfg(test)]
static TEST_MUTEX: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Register the per-kind unknown-fields source. Called from the same three
/// resource_controller call sites as `status_metrics::register_status_renderer`,
/// right after the `RawStore` is wired.
///
/// A no-op for every kind except `EdgionPlugins` / `EdgionStreamPlugins` —
/// the only two resources carrying an `unknown_fields` field — so the other
/// ~20 kinds pay only a string compare and never occupy a registry slot.
pub(crate) fn register_unknown_fields_source<K>(kind: &'static str, store: Arc<dyn RawStore<K>>)
where
    K: Send + Sync + 'static,
{
    let source: Source = match kind {
        "EdgionPlugins" => Box::new(move |out: &mut Vec<UnknownFieldsEntry>| {
            for obj in store.snapshot() {
                let any: Arc<dyn Any + Send + Sync> = obj;
                let Ok(plugins) = any.downcast::<edgion_resources::EdgionPlugins>() else {
                    continue;
                };
                if !plugins.spec.unknown_fields.is_empty() {
                    out.push(UnknownFieldsEntry {
                        kind: "EdgionPlugins",
                        namespace: plugins.metadata.namespace.clone().unwrap_or_default(),
                        name: plugins.metadata.name.clone().unwrap_or_default(),
                        paths: plugins.spec.unknown_fields.clone(),
                    });
                }
            }
        }),
        "EdgionStreamPlugins" => Box::new(move |out: &mut Vec<UnknownFieldsEntry>| {
            for obj in store.snapshot() {
                let any: Arc<dyn Any + Send + Sync> = obj;
                let Ok(plugins) = any.downcast::<edgion_resources::EdgionStreamPlugins>() else {
                    continue;
                };
                if !plugins.spec.unknown_fields.is_empty() {
                    out.push(UnknownFieldsEntry {
                        kind: "EdgionStreamPlugins",
                        namespace: plugins.metadata.namespace.clone().unwrap_or_default(),
                        name: plugins.metadata.name.clone().unwrap_or_default(),
                        paths: plugins.spec.unknown_fields.clone(),
                    });
                }
            }
        }),
        _ => return,
    };
    // Keyed by `kind`: a reload-time second call for the same kind drops the
    // prior closure (and with it the Arc to the old RawStore), matching
    // `register_status_renderer`'s re-registration contract.
    sources_slot().lock().insert(kind, source);
}

/// Run every registered source, collect findings, sort for determinism, and
/// cap the result. Returns `(total_before_cap, truncated, items)`.
///
/// Sorting mirrors the walker's own determinism rule
/// (`schema_check` / `05-issues.md` R-5): output must not depend on
/// `HashMap` iteration order or on `RawStore::snapshot`'s unspecified order.
pub(crate) fn collect_unknown_fields(cap: usize) -> (usize, bool, Vec<UnknownFieldsEntry>) {
    let mut all = Vec::new();
    {
        let sources = sources_slot().lock();
        for source in sources.values() {
            source(&mut all);
        }
    }
    all.sort_by(|a, b| {
        (a.kind, a.namespace.as_str(), a.name.as_str()).cmp(&(b.kind, b.namespace.as_str(), b.name.as_str()))
    });
    let total = all.len();
    let truncated = total > cap;
    all.truncate(cap);
    (total, truncated, all)
}

// ---------------------------------------------------------------------
// `edgion_controller_unknown_config_fields` gauge.
//
// Deliberately built on top of `collect_unknown_fields` above (the live
// `spec.unknown_fields` scan), NOT on `status_metrics::classify()`.
// `edgion_controller_resource_abnormal` is derived from resource STATUS
<<<<<<< Updated upstream
// CONDITIONS, which per the controller-metrics section of
// `docs/zh-CN/ops-guide/plugin-policy.md` makes that gauge Kubernetes-mode only: in FileSystem/etcd
=======
// CONDITIONS, which per `docs/zh-CN/ops-guide/plugin-policy.md` ("Controller
// "Controller metrics" section) makes that gauge Kubernetes-mode only: in FileSystem/etcd
>>>>>>> Stashed changes
// mode the scanner reads pre-processed objects that never carry status
// conditions at all. Our findings live on `spec.unknown_fields`, which
// `collect_unknown_fields` reads straight from the typed `RawStore` in ALL
// THREE conf_center modes. Routing this metric through `classify()` would
// silently produce zero coverage in FileSystem/etcd mode — see
// `status_metrics.rs`'s `classify()` doc comment and the pin test
// `unknown_config_fields_true_produces_no_series`, both of which this
// metric must NOT disturb: it is a separate gauge, on purpose.
//
// Like `edgion_controller_resource_abnormal`, there is no background loop:
// `collect_unknown_fields` is already a cheap on-demand scan (used
// synchronously by the diagnostics handler), so the `/metrics` handler
// just calls it again at scrape time.

pub(crate) const METRIC_NAME: &str = "edgion_controller_unknown_config_fields";
pub(crate) const METRIC_HELP: &str = "Resources whose submitted document contains a key the EdgionPlugins/EdgionStreamPlugins schema does not recognize (see GET /api/v1/diagnostics/unknown-fields), aggregated ONE SERIES PER KIND. The value is the number of affected resources of that kind. The `warn` label is the full finding set as a JSON array of {namespace,name,paths}, sorted deterministically, capped at a fixed byte budget with a visible truncation marker object appended when it does not all fit; per-resource `paths` are separately capped with a visible `(+M more)` marker. Emitted ONLY for kinds with at least one finding, so a healthy fleet emits nothing. Advisory only — does not affect Accepted, never blocks a sync.";

/// Byte budget for the whole `warn` label value (the serialized JSON array),
/// per kind. Chosen in the low kilobytes: large enough that a realistic
/// number of findings round-trips without truncation in normal operation,
/// small enough to keep one Prometheus sample line — and the scrape as a
/// whole, since at most two of these series exist — comfortably bounded
/// even on a badly misconfigured fleet. Unlike the old per-resource-series
/// design, cardinality is now fixed at <= 2 series regardless of fleet
/// size, so this budget is the only remaining unbounded-payload risk and is
/// enforced by [`render_kind_payload`].
const LABEL_BYTE_BUDGET: usize = 4096;

/// Cap on how many finding paths are embedded in one resource's `paths`
/// array before the rest are summarized with a visible `(+M more)` marker.
/// Mirrors the capping idea in
/// `resource_processor/unknown_fields_stamp.rs::format_message` — a single
/// malformed resource can carry up to `MAX_JSON_INSTANCE_NODES` (20000)
/// findings, and an uncapped per-resource path list would alone be able to
/// exhaust [`LABEL_BYTE_BUDGET`].
const MAX_PATHS_PER_RESOURCE: usize = 20;

/// Sanity cap on how many resources `collect_unknown_fields` is asked to
/// return before per-kind grouping. Effectively "everything" for any
/// realistic fleet — the real enforcement point is
/// [`LABEL_BYTE_BUDGET`] inside [`render_kind_payload`], which caps what
/// actually lands in the label value; this outer cap exists only so a
/// pathological fleet size can't make the scan itself unbounded.
const RESOURCE_SCAN_CAP: usize = 100_000;

/// One resource's contribution to a kind's `warn` payload: its `paths` are
/// capped and visibly marked, exactly like the per-resource label design
/// this replaced.
fn capped_paths_value(paths: &[String]) -> Vec<String> {
    if paths.len() > MAX_PATHS_PER_RESOURCE {
        let mut shown: Vec<String> = paths[..MAX_PATHS_PER_RESOURCE].to_vec();
        shown.push(format!("(+{} more)", paths.len() - MAX_PATHS_PER_RESOURCE));
        shown
    } else {
        paths.to_vec()
    }
}

/// Build one kind's `warn` label JSON value from its (already
/// namespace/name-sorted, by `collect_unknown_fields`) entries.
///
/// The array holds `{namespace,name,paths}` objects until adding the next
/// one would exceed [`LABEL_BYTE_BUDGET`]; if any resources are left over,
/// a trailing sentinel object — `{"truncated":true,"omittedResources":N}` —
/// is appended so truncation is visible inside the JSON itself rather than
/// silently dropped. A sentinel object has no `namespace`/`name` fields, so
/// a consumer can distinguish it from a real finding by shape.
///
/// Deterministic: same input always serializes to the identical byte
/// sequence (entries pre-sorted by the caller, no HashMap iteration here),
/// so an unchanged fleet does not mint a new label value — and therefore
/// not a new series identity — on every scrape.
fn render_kind_payload(kind: &str, entries: &[&UnknownFieldsEntry]) -> String {
    let mut arr: Vec<serde_json::Value> = Vec::with_capacity(entries.len());
    let mut included = 0usize;
    // "[]" baseline plus headroom for the trailing sentinel object, so the
    // budget check below never has to retroactively evict an already-added
    // entry to make room for the truncation marker.
    let sentinel_headroom = 64usize;
    let mut len_estimate = 2usize;
    for entry in entries {
        let obj = serde_json::json!({
            "namespace": entry.namespace,
            "name": entry.name,
            "paths": capped_paths_value(&entry.paths),
        });
        let obj_len = serde_json::to_string(&obj).map(|s| s.len()).unwrap_or(0);
        let projected = len_estimate + obj_len + 1; // +1 for the separating comma
        if included > 0 && projected + sentinel_headroom > LABEL_BYTE_BUDGET {
            break;
        }
        arr.push(obj);
        len_estimate = projected;
        included += 1;
    }
    let omitted = entries.len() - included;
    if omitted > 0 {
        tracing::error!(
            component = "unknown_fields_metrics",
            kind = kind,
            included = included,
            omitted = omitted,
            budget_bytes = LABEL_BYTE_BUDGET,
            "unknown-config-fields warn payload exceeds byte budget; truncating"
        );
        arr.push(serde_json::json!({
            "truncated": true,
            "omittedResources": omitted,
        }));
    }
    serde_json::to_string(&arr).unwrap_or_else(|err| {
        tracing::warn!(
            component = "unknown_fields_metrics",
            kind = kind,
            error = %err,
            "failed to serialize unknown-fields warn payload; emitting empty array"
        );
        "[]".to_string()
    })
}

/// Render the `edgion_controller_unknown_config_fields` Prometheus text
/// block: one series per kind (`EdgionPlugins` / `EdgionStreamPlugins`),
/// never per resource. Returns "" when there are no findings anywhere, so
/// the caller can skip appending entirely (no bare `# HELP`/`# TYPE` with
/// zero samples).
///
/// Only a `warn` payload exists today (this feature is warn-only by
/// design — it never blocks or rejects); an `err` payload could be added
/// later as a second label without reshaping the one-series-per-kind
/// design, so this function deliberately does not emit an empty `err=""`
/// label just for symmetry with `edgion_controller_resource_abnormal`.
pub(crate) fn render_unknown_config_fields_metric() -> String {
    let (total, scan_truncated, items) = collect_unknown_fields(RESOURCE_SCAN_CAP);
    if items.is_empty() {
        return String::new();
    }
    if scan_truncated {
        tracing::error!(
            component = "unknown_fields_metrics",
            total = total,
            cap = RESOURCE_SCAN_CAP,
            "unknown-config-fields resource scan exceeds sanity cap; per-kind counts will undercount"
        );
    }

    // Group by kind. `items` is already sorted by (kind, namespace, name)
    // (see `collect_unknown_fields`), and `BTreeMap` iterates keys in
    // sorted order, so both the kind emission order and each kind's
    // resource order stay deterministic without a separate sort here.
    let mut by_kind: std::collections::BTreeMap<&'static str, Vec<&UnknownFieldsEntry>> =
        std::collections::BTreeMap::new();
    for entry in &items {
        by_kind.entry(entry.kind).or_default().push(entry);
    }

    let mut out = String::with_capacity(256 + by_kind.len() * (LABEL_BYTE_BUDGET + 64));
    out.push_str("# HELP ");
    out.push_str(METRIC_NAME);
    out.push(' ');
    out.push_str(METRIC_HELP);
    out.push('\n');
    out.push_str("# TYPE ");
    out.push_str(METRIC_NAME);
    out.push_str(" gauge\n");
    for (kind, entries) in &by_kind {
        let warn_json = render_kind_payload(kind, entries);
        out.push_str(METRIC_NAME);
        out.push_str("{kind=\"");
        out.push_str(kind);
        out.push_str("\",warn=\"");
        crate::observe::status_metrics::append_escaped(&mut out, &warn_json);
        out.push_str("\"} ");
        out.push_str(&entries.len().to_string());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf_mgr::sync_runtime::resource_processor::LocalRawStore;
    use edgion_resources::{EdgionPlugins, EdgionPluginsSpec, EdgionStreamPlugins, EdgionStreamPluginsSpec};

    fn reset() {
        sources_slot().lock().clear();
    }

    fn plugins_with_findings(ns: &str, name: &str, paths: Vec<String>) -> Arc<EdgionPlugins> {
        let mut ep = EdgionPlugins::new(name, EdgionPluginsSpec::default());
        ep.metadata.namespace = Some(ns.to_string());
        ep.spec.unknown_fields = paths;
        Arc::new(ep)
    }

    fn stream_plugins_with_findings(ns: &str, name: &str, paths: Vec<String>) -> Arc<EdgionStreamPlugins> {
        let mut esp = EdgionStreamPlugins::new(
            name,
            EdgionStreamPluginsSpec {
                plugins: None,
                tls_route_plugins: None,
                unknown_fields: paths,
            },
        );
        esp.metadata.namespace = Some(ns.to_string());
        Arc::new(esp)
    }

    #[test]
    fn empty_registry_yields_no_findings() {
        let _guard = TEST_MUTEX.lock();
        reset();
        let (total, truncated, items) = collect_unknown_fields(100);
        assert_eq!(total, 0);
        assert!(!truncated);
        assert!(items.is_empty());
    }

    #[test]
    fn reports_findings_from_both_plugin_kinds() {
        let _guard = TEST_MUTEX.lock();
        reset();

        let plugins_store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
        plugins_store.upsert(
            "default/auth-chain".into(),
            plugins_with_findings("default", "auth-chain", vec!["requestPlugins[0].config.denys".into()]),
        );
        register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", plugins_store);

        let stream_store = Arc::new(LocalRawStore::<EdgionStreamPlugins>::new());
        stream_store.upsert(
            "default/geo".into(),
            stream_plugins_with_findings("default", "geo", vec!["plugins[0].config.typo".into()]),
        );
        register_unknown_fields_source::<EdgionStreamPlugins>("EdgionStreamPlugins", stream_store);

        let (total, truncated, items) = collect_unknown_fields(100);
        assert_eq!(total, 2);
        assert!(!truncated);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].kind, "EdgionPlugins");
        assert_eq!(items[0].paths, vec!["requestPlugins[0].config.denys".to_string()]);
        assert_eq!(items[1].kind, "EdgionStreamPlugins");
        assert_eq!(items[1].paths, vec!["plugins[0].config.typo".to_string()]);
    }

    #[test]
    fn clean_resources_are_not_reported() {
        let _guard = TEST_MUTEX.lock();
        reset();

        let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
        store.upsert(
            "default/clean".into(),
            plugins_with_findings("default", "clean", Vec::new()),
        );
        register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", store);

        let (total, truncated, items) = collect_unknown_fields(100);
        assert_eq!(total, 0);
        assert!(!truncated);
        assert!(items.is_empty());
    }

    #[test]
    fn other_kinds_are_ignored_without_registering_a_source() {
        let _guard = TEST_MUTEX.lock();
        reset();

        // A kind with no `unknown_fields` field: register_unknown_fields_source
        // must be a cheap no-op, never occupying a registry slot.
        let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
        register_unknown_fields_source::<EdgionPlugins>("HTTPRoute", store);
        assert!(sources_slot().lock().is_empty());
    }

    #[test]
    fn cap_truncates_and_reports_honestly() {
        let _guard = TEST_MUTEX.lock();
        reset();

        let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
        for i in 0..5 {
            store.upsert(
                format!("default/p{i}"),
                plugins_with_findings("default", &format!("p{i}"), vec!["typo".into()]),
            );
        }
        register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", store);

        let (total, truncated, items) = collect_unknown_fields(2);
        assert_eq!(total, 5, "total must report the pre-cap count");
        assert!(truncated);
        assert_eq!(items.len(), 2, "items must honor the cap");
    }

    /// Guards against the conf_center-reload duplication bug (see the module
    /// doc comment): a second `register_unknown_fields_source` call for the
    /// same kind must replace the prior source, not accumulate a duplicate.
    #[test]
    fn re_registering_same_kind_replaces_old_source() {
        let _guard = TEST_MUTEX.lock();
        reset();

        let old_store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
        old_store.upsert(
            "default/old".into(),
            plugins_with_findings("default", "old", vec!["old.path".into()]),
        );
        register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", old_store);

        // Simulated reload: second registration for the SAME kind with a
        // freshly-constructed store exposing a differently-named resource.
        let new_store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
        new_store.upsert(
            "default/new".into(),
            plugins_with_findings("default", "new", vec!["new.path".into()]),
        );
        register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", new_store);

        let (total, truncated, items) = collect_unknown_fields(100);
        assert_eq!(total, 1, "expected exactly 1 entry (old source replaced)");
        assert!(!truncated);
        assert_eq!(items[0].name, "new");
    }

    mod metric_rendering {
        use super::*;

        /// Undo Prometheus label-value escaping (`\\` -> `\`, `\"` -> `"`),
        /// returning the raw JSON string the label carried.
        fn unescape_label(label_value: &str) -> String {
            let mut out = String::new();
            let mut chars = label_value.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    match chars.next() {
                        Some('\\') => out.push('\\'),
                        Some('"') => out.push('"'),
                        Some('n') => out.push('\n'),
                        Some(other) => out.push(other),
                        None => {}
                    }
                } else {
                    out.push(c);
                }
            }
            out
        }

        /// Extract the `warn="..."` label value (still Prometheus-escaped)
        /// from a rendered series line.
        fn warn_label_of(line: &str) -> &str {
            let start = line.find("warn=\"").unwrap() + "warn=\"".len();
            let end = line.rfind("\"} ").unwrap();
            &line[start..end]
        }

        #[test]
        fn healthy_fleet_emits_no_series_and_no_block() {
            let _guard = TEST_MUTEX.lock();
            reset();

            let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
            store.upsert(
                "default/clean".into(),
                plugins_with_findings("default", "clean", Vec::new()),
            );
            register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", store);

            let block = render_unknown_config_fields_metric();
            assert!(
                block.is_empty(),
                "a healthy fleet must render nothing, not a bare HELP/TYPE: {block:?}"
            );
        }

        #[test]
        fn one_finding_renders_one_series_per_kind_with_resource_count_as_value() {
            let _guard = TEST_MUTEX.lock();
            reset();

            let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
            store.upsert(
                "default/demo-bad".into(),
                plugins_with_findings("default", "demo-bad", vec!["requestPlugins[0].config.denys".into()]),
            );
            register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", store);

            let block = render_unknown_config_fields_metric();
            let expected = concat!(
                "# HELP edgion_controller_unknown_config_fields ",
                "Resources whose submitted document contains a key the EdgionPlugins/EdgionStreamPlugins schema does not recognize (see GET /api/v1/diagnostics/unknown-fields), aggregated ONE SERIES PER KIND. The value is the number of affected resources of that kind. The `warn` label is the full finding set as a JSON array of {namespace,name,paths}, sorted deterministically, capped at a fixed byte budget with a visible truncation marker object appended when it does not all fit; per-resource `paths` are separately capped with a visible `(+M more)` marker. Emitted ONLY for kinds with at least one finding, so a healthy fleet emits nothing. Advisory only — does not affect Accepted, never blocks a sync.\n",
                "# TYPE edgion_controller_unknown_config_fields gauge\n",
                "edgion_controller_unknown_config_fields{kind=\"EdgionPlugins\",warn=\"[{\\\"name\\\":\\\"demo-bad\\\",\\\"namespace\\\":\\\"default\\\",\\\"paths\\\":[\\\"requestPlugins[0].config.denys\\\"]}]\"} 1\n",
            );
            assert_eq!(block, expected);
        }

        #[test]
        fn two_affected_resources_of_the_same_kind_aggregate_into_one_series() {
            let _guard = TEST_MUTEX.lock();
            reset();

            let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
            store.upsert(
                "default/demo-bad".into(),
                plugins_with_findings("default", "demo-bad", vec!["requestPlugins[0].config.denys".into()]),
            );
            store.upsert(
                "default/other-bad".into(),
                plugins_with_findings("default", "other-bad", vec!["plugins[0].config.typo".into()]),
            );
            register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", store);

            let block = render_unknown_config_fields_metric();
            let samples: Vec<&str> = block.lines().filter(|l| !l.starts_with('#')).collect();
            assert_eq!(samples.len(), 1, "one kind must still be exactly one series: {block}");
            let line = samples[0];
            assert!(line.starts_with("edgion_controller_unknown_config_fields{kind=\"EdgionPlugins\""));
            assert!(
                line.ends_with("} 2\n") || line.ends_with("} 2"),
                "value must be the affected-resource count: {line}"
            );

            let unescaped = unescape_label(warn_label_of(line));
            let parsed: serde_json::Value = serde_json::from_str(&unescaped).expect("warn label must be valid JSON");
            let arr = parsed.as_array().expect("warn label must be a JSON array");
            assert_eq!(arr.len(), 2);
            // Sorted by namespace then name: "demo-bad" before "other-bad".
            assert_eq!(arr[0]["name"], "demo-bad");
            assert_eq!(arr[1]["name"], "other-bad");
        }

        #[test]
        fn two_kinds_each_get_their_own_series() {
            let _guard = TEST_MUTEX.lock();
            reset();

            let plugins_store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
            plugins_store.upsert(
                "default/auth-chain".into(),
                plugins_with_findings("default", "auth-chain", vec!["requestPlugins[0].config.denys".into()]),
            );
            register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", plugins_store);

            let stream_store = Arc::new(LocalRawStore::<EdgionStreamPlugins>::new());
            stream_store.upsert(
                "default/geo".into(),
                stream_plugins_with_findings("default", "geo", vec!["plugins[0].config.typo".into()]),
            );
            register_unknown_fields_source::<EdgionStreamPlugins>("EdgionStreamPlugins", stream_store);

            let block = render_unknown_config_fields_metric();
            let samples: Vec<&str> = block.lines().filter(|l| !l.starts_with('#')).collect();
            assert_eq!(samples.len(), 2, "at most one series per kind: {block}");
            assert!(samples[0].starts_with("edgion_controller_unknown_config_fields{kind=\"EdgionPlugins\""));
            assert!(samples[1].starts_with("edgion_controller_unknown_config_fields{kind=\"EdgionStreamPlugins\""));
        }

        #[test]
        fn quote_and_backslash_in_path_are_escaped() {
            let _guard = TEST_MUTEX.lock();
            reset();

            let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
            store.upsert(
                "default/weird".into(),
                plugins_with_findings("default", "weird", vec![r#"config."quoted\field""#.into()]),
            );
            register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", store);

            let block = render_unknown_config_fields_metric();
            let line = block.lines().last().unwrap();
            let unescaped = unescape_label(warn_label_of(line));
            let parsed: serde_json::Value = serde_json::from_str(&unescaped).expect("warn label must be valid JSON");
            let paths = parsed[0]["paths"].as_array().expect("paths must be an array");
            assert_eq!(paths.len(), 1);
            assert_eq!(paths[0], r#"config."quoted\field""#);
        }

        #[test]
        fn per_resource_paths_truncate_past_the_cap_with_visible_marker() {
            let _guard = TEST_MUTEX.lock();
            reset();

            let many: Vec<String> = (0..(MAX_PATHS_PER_RESOURCE + 7)).map(|i| format!("path{i}")).collect();
            let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
            store.upsert(
                "default/many".into(),
                plugins_with_findings("default", "many", many.clone()),
            );
            register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", store);

            let block = render_unknown_config_fields_metric();
            let line = block.lines().last().unwrap();
            let unescaped = unescape_label(warn_label_of(line));
            let parsed: serde_json::Value = serde_json::from_str(&unescaped).expect("warn label must be valid JSON");
            let paths = parsed[0]["paths"].as_array().expect("paths must be an array");
            assert_eq!(
                paths.len(),
                MAX_PATHS_PER_RESOURCE + 1,
                "shown paths + one marker entry"
            );
            assert_eq!(paths[MAX_PATHS_PER_RESOURCE], "(+7 more)");
            for (i, p) in many.iter().take(MAX_PATHS_PER_RESOURCE).enumerate() {
                assert_eq!(paths[i], p.as_str());
            }
        }

        #[test]
        fn resource_count_past_byte_budget_truncates_the_payload_with_visible_marker() {
            let _guard = TEST_MUTEX.lock();
            reset();

            // Each resource contributes roughly 60-70 bytes of JSON (short
            // namespace/name, one short path); with a 4096-byte budget this
            // comfortably overflows well before 1000 resources.
            let resource_count = 200;
            let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
            for i in 0..resource_count {
                store.upsert(
                    format!("default/p{i:04}"),
                    plugins_with_findings("default", &format!("p{i:04}"), vec!["config.typo".into()]),
                );
            }
            register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", store);

            let block = render_unknown_config_fields_metric();
            let line = block.lines().last().unwrap();
            // The gauge VALUE must report the true affected-resource count,
            // regardless of how much of the JSON payload was truncated.
            assert!(
                line.ends_with(&format!("}} {resource_count}")) || line.ends_with(&format!("}} {resource_count}\n")),
                "value must be the untruncated resource count: {line}"
            );

            let unescaped = unescape_label(warn_label_of(line));
            let parsed: serde_json::Value = serde_json::from_str(&unescaped).expect("warn label must be valid JSON");
            let arr = parsed.as_array().expect("warn label must be a JSON array");
            let last = arr.last().expect("payload must be non-empty");
            assert_eq!(
                last["truncated"], true,
                "a truncated payload must end with a visible {{truncated:true}} marker, got {arr:#?}"
            );
            let omitted = last["omittedResources"]
                .as_u64()
                .expect("omittedResources must be present");
            assert_eq!(
                arr.len() as u64 - 1 + omitted,
                resource_count as u64,
                "shown entries + omitted count must equal the true total"
            );
            assert!(omitted > 0, "test setup must actually exceed the budget");
        }

        /// Pin: rendering an unchanged fleet twice must produce byte-identical
        /// output. A non-deterministic label value would mint a brand-new
        /// series identity on every scrape (far worse than the per-resource
        /// cardinality this per-kind design was built to avoid).
        #[test]
        fn rendering_is_deterministic_across_repeated_calls() {
            let _guard = TEST_MUTEX.lock();
            reset();

            let plugins_store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
            for i in 0..30 {
                plugins_store.upsert(
                    format!("default/p{i:03}"),
                    plugins_with_findings("default", &format!("p{i:03}"), vec![format!("config.typo{i}")]),
                );
            }
            register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", plugins_store);

            let stream_store = Arc::new(LocalRawStore::<EdgionStreamPlugins>::new());
            stream_store.upsert(
                "default/geo".into(),
                stream_plugins_with_findings("default", "geo", vec!["plugins[0].config.typo".into()]),
            );
            register_unknown_fields_source::<EdgionStreamPlugins>("EdgionStreamPlugins", stream_store);

            let first = render_unknown_config_fields_metric();
            let second = render_unknown_config_fields_metric();
            assert_eq!(
                first, second,
                "repeated rendering of an unchanged fleet must be byte-identical"
            );
        }

        /// Pin: this metric must remain independent of
        /// `status_metrics::classify()` / the abnormal-gauge pin test —
        /// a resource carrying only `UnknownConfigFields=True` on its
        /// status conditions produces no `resource_abnormal` series (see
        /// `status_metrics_test.rs::unknown_config_fields_true_produces_no_series`),
        /// while this metric is driven purely by `spec.unknown_fields`
        /// findings and knows nothing about status conditions at all.
        #[test]
        fn metric_is_independent_of_status_conditions() {
            let _guard = TEST_MUTEX.lock();
            reset();
            // No source registered at all: no status conditions exist
            // anywhere in this test, and the metric must still render
            // correctly from spec-level findings alone.
            let store = Arc::new(LocalRawStore::<EdgionPlugins>::new());
            store.upsert(
                "default/spec-only".into(),
                plugins_with_findings("default", "spec-only", vec!["config.typo".into()]),
            );
            register_unknown_fields_source::<EdgionPlugins>("EdgionPlugins", store);

            let block = render_unknown_config_fields_metric();
            assert!(block.contains("edgion_controller_unknown_config_fields{kind=\"EdgionPlugins\""));
        }
    }
}
