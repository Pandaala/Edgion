//! `edgion_controller_resource_abnormal` gauge.
//!
//! See `docs/superpowers/specs/2026-04-20-controller-resource-abnormal-metrics-design.md`
//! for the full design. Short version:
//!
//! * A background task rebuilds a fresh `Vec<Entry>` every `tick_secs` by
//!   reading every registered `RawStore`, serializing each resource to JSON,
//!   recursively scanning for `"conditions": [...]` arrays, and classifying
//!   into `Err` / `Warn` based on condition type.
//! * The snapshot is published atomically via `ArcSwap<Arc<Vec<Entry>>>`.
//! * `/metrics` is read-only; it loads the current snapshot and appends
//!   a text block to the common Prometheus handler output.
//! * No incremental state, no per-resource hooks — every tick fully
//!   replaces the Vec, so recovery and deletion are automatic.
//!
//! Cardinality: one series per abnormal resource at most. Hard safety cap
//! of 10 000 entries per snapshot (see [`SNAPSHOT_ENTRY_HARD_CAP`]).

use arc_swap::ArcSwap;
use parking_lot::Mutex;
use serde::Serialize;
use std::sync::{Arc, OnceLock};

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::raw_store::RawStore;

pub const METRIC_NAME: &str = "edgion_controller_resource_abnormal";
pub const METRIC_HELP: &str =
    "Resource currently has at least one False status condition. type=err for Accepted/ResolvedRefs/Programmed failures (or Conflicted/ListenersNotValid=True), type=warn for Ready:False or PartiallyInvalid:True.";

/// Safety ceiling: if a snapshot would exceed this, it is truncated and
/// a single `tracing::error!` is emitted. Hard-coded — not a config knob.
pub const SNAPSHOT_ENTRY_HARD_CAP: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Kind {
    Err,
    Warn,
}

impl Kind {
    #[inline]
    pub fn as_label(self) -> &'static str {
        match self {
            Kind::Err => "err",
            Kind::Warn => "warn",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    /// K8s `.kind` field (PascalCase singular), e.g. `HTTPRoute`.
    pub kind: &'static str,
    pub namespace: String,
    pub name: String,
    pub ty: Kind,
}

/// Renderer closure signature: pushes `Entry` values for one kind's store.
pub type Renderer = Box<dyn Fn(&mut Vec<Entry>) + Send + Sync>;

/// Global current snapshot. Read lock-free on every `/metrics` scrape;
/// replaced atomically at the end of each background tick.
pub fn snapshot_slot() -> &'static ArcSwap<Vec<Entry>> {
    static SNAPSHOT: OnceLock<ArcSwap<Vec<Entry>>> = OnceLock::new();
    SNAPSHOT.get_or_init(|| ArcSwap::from_pointee(Vec::new()))
}

/// Registered per-kind renderers. Append-only during controller startup;
/// each tick's loop body briefly locks to iterate.
pub fn renderers_slot() -> &'static Mutex<Vec<Renderer>> {
    static RENDERERS: OnceLock<Mutex<Vec<Renderer>>> = OnceLock::new();
    RENDERERS.get_or_init(|| Mutex::new(Vec::new()))
}

/// A condition as observed from serialized resource JSON. Only the fields
/// we actually classify on.
#[derive(Debug, Clone)]
pub struct Condition {
    pub type_: String,
    pub status: String,
}

/// Classify a resource's full set of conditions into err / warn / clean.
///
/// Returns `Some(Err)` if *any* condition triggers the err rule; `Some(Warn)`
/// only if no err was present and at least one warn-trigger exists; `None`
/// for an all-clean or empty set.
///
/// Err triggers:
/// - `Accepted:False`, `ResolvedRefs:False`, `Programmed:False`
/// - `Conflicted:True`, `ListenersNotValid:True` (inverse semantics —
///   True is bad for these two)
///
/// Warn triggers:
/// - `Ready:False`
/// - `PartiallyInvalid:True` (inverse)
pub fn classify(conditions: &[Condition]) -> Option<Kind> {
    let mut warn_seen = false;
    for c in conditions {
        match (c.type_.as_str(), c.status.as_str()) {
            ("Accepted", "False")
            | ("ResolvedRefs", "False")
            | ("Programmed", "False")
            | ("Conflicted", "True")
            | ("ListenersNotValid", "True") => return Some(Kind::Err),

            ("Ready", "False") | ("PartiallyInvalid", "True") => {
                warn_seen = true;
            }
            _ => {}
        }
    }
    warn_seen.then_some(Kind::Warn)
}

#[cfg(test)]
mod classify_tests {
    use super::*;

    fn c(t: &str, s: &str) -> Condition {
        Condition { type_: t.into(), status: s.into() }
    }

    #[test]
    fn empty_is_clean() {
        assert_eq!(classify(&[]), None);
    }

    #[test]
    fn all_true_is_clean() {
        let cs = vec![c("Accepted", "True"), c("ResolvedRefs", "True"), c("Programmed", "True")];
        assert_eq!(classify(&cs), None);
    }

    #[test]
    fn accepted_false_is_err() {
        assert_eq!(classify(&[c("Accepted", "False")]), Some(Kind::Err));
    }

    #[test]
    fn resolved_refs_false_is_err() {
        assert_eq!(classify(&[c("ResolvedRefs", "False")]), Some(Kind::Err));
    }

    #[test]
    fn programmed_false_is_err() {
        assert_eq!(classify(&[c("Programmed", "False")]), Some(Kind::Err));
    }

    #[test]
    fn conflicted_true_is_err() {
        assert_eq!(classify(&[c("Conflicted", "True")]), Some(Kind::Err));
    }

    #[test]
    fn listeners_not_valid_true_is_err() {
        assert_eq!(classify(&[c("ListenersNotValid", "True")]), Some(Kind::Err));
    }

    #[test]
    fn partially_invalid_true_is_warn() {
        assert_eq!(classify(&[c("PartiallyInvalid", "True")]), Some(Kind::Warn));
    }

    #[test]
    fn ready_false_is_warn() {
        assert_eq!(classify(&[c("Ready", "False")]), Some(Kind::Warn));
    }

    #[test]
    fn err_and_warn_together_swallow_warn() {
        let cs = vec![c("Accepted", "False"), c("Ready", "False")];
        assert_eq!(classify(&cs), Some(Kind::Err));
    }

    #[test]
    fn unknown_type_is_ignored() {
        let cs = vec![c("SomethingElse", "False"), c("Accepted", "True")];
        assert_eq!(classify(&cs), None);
    }

    #[test]
    fn unknown_status_is_ignored() {
        assert_eq!(classify(&[c("Accepted", "Unknown")]), None);
    }
}

/// Recursively walk a JSON value and collect every entry in any
/// `"conditions": [ ... ]` array into a flat `Vec<Condition>`.
///
/// Covers:
/// - resource-level `.status.conditions`
/// - per-parent `.status.parents[*].conditions`
/// - per-listener `.status.listeners[*].conditions`
/// - any other future nesting with the `conditions` key + array value
///
/// Malformed entries (missing `type` or non-string `status`) are skipped
/// silently.
pub fn scan_conditions(value: &serde_json::Value) -> Vec<Condition> {
    let mut out = Vec::new();
    walk(value, &mut out);
    out
}

fn walk(value: &serde_json::Value, out: &mut Vec<Condition>) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                if k == "conditions" {
                    if let serde_json::Value::Array(arr) = v {
                        for item in arr {
                            if let Some(cond) = extract_condition(item) {
                                out.push(cond);
                            }
                        }
                    }
                } else {
                    walk(v, out);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                walk(item, out);
            }
        }
        _ => {}
    }
}

fn extract_condition(v: &serde_json::Value) -> Option<Condition> {
    let obj = v.as_object()?;
    let type_ = obj.get("type")?.as_str()?.to_string();
    let status = obj.get("status")?.as_str()?.to_string();
    Some(Condition { type_, status })
}

/// Render a sorted snapshot to Prometheus text format. Returns empty
/// string on empty input so the wrapper handler can skip appending
/// entirely.
pub fn render_snapshot(entries: &[Entry]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(128 + entries.len() * 96);
    out.push_str("# HELP ");
    out.push_str(METRIC_NAME);
    out.push(' ');
    out.push_str(METRIC_HELP);
    out.push('\n');
    out.push_str("# TYPE ");
    out.push_str(METRIC_NAME);
    out.push_str(" gauge\n");
    for e in entries {
        out.push_str(METRIC_NAME);
        out.push_str("{kind=\"");
        out.push_str(e.kind);
        out.push_str("\",namespace=\"");
        append_escaped(&mut out, &e.namespace);
        out.push_str("\",name=\"");
        append_escaped(&mut out, &e.name);
        out.push_str("\",type=\"");
        out.push_str(e.ty.as_label());
        out.push_str("\"} 1\n");
    }
    out
}

/// Escape `\` and `"` per Prometheus text-format conventions.
fn append_escaped(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
}

/// Scan one kind's store and push any abnormal resources into `out`.
///
/// Expects K to serialize to JSON with at least:
/// - `.metadata.namespace` (string; "" for cluster-scoped resources)
/// - `.metadata.name` (string)
/// - `.status.conditions[]` or nested variants (handled by
///   [`scan_conditions`])
///
/// Any resource that fails to serialize is skipped with a single
/// `tracing::warn!`; we do not propagate the failure.
pub fn render_kind<K>(kind: &'static str, store: &dyn RawStore<K>, out: &mut Vec<Entry>)
where
    K: serde::Serialize,
{
    for obj in store.snapshot() {
        let json = match serde_json::to_value(&*obj) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    component = "status_metrics",
                    kind = kind,
                    error = %e,
                    "serialize failed; skipping resource in snapshot"
                );
                continue;
            }
        };

        let (ns, name) = extract_identity(&json);
        let conds = scan_conditions(&json);
        if let Some(ty) = classify(&conds) {
            out.push(Entry { kind, namespace: ns, name, ty });
        }
    }
}

fn extract_identity(json: &serde_json::Value) -> (String, String) {
    let meta = json.get("metadata");
    let ns = meta
        .and_then(|m| m.get("namespace"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let name = meta
        .and_then(|m| m.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    (ns, name)
}

/// Register a per-kind renderer with the global registry. Called by each
/// resource_controller's startup path, right after it wires its
/// `RawStore` into the processor.
pub fn register_status_renderer<K>(kind: &'static str, store: Arc<dyn RawStore<K>>)
where
    K: serde::Serialize + Send + Sync + 'static,
{
    let renderer: Renderer = Box::new(move |buf: &mut Vec<Entry>| {
        render_kind(kind, store.as_ref(), buf);
    });
    renderers_slot().lock().push(renderer);
}

#[cfg(test)]
mod scan_conditions_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resource_level_conditions() {
        let j = json!({
            "status": {
                "conditions": [
                    {"type": "Accepted", "status": "False", "reason": "Invalid"}
                ]
            }
        });
        let conds = scan_conditions(&j);
        assert_eq!(conds.len(), 1);
        assert_eq!(conds[0].type_, "Accepted");
        assert_eq!(conds[0].status, "False");
    }

    #[test]
    fn nested_parents_conditions() {
        let j = json!({
            "status": {
                "parents": [
                    {"conditions": [{"type": "Accepted", "status": "True"}]},
                    {"conditions": [{"type": "ResolvedRefs", "status": "False"}]}
                ]
            }
        });
        let conds = scan_conditions(&j);
        assert_eq!(conds.len(), 2);
    }

    #[test]
    fn gateway_top_level_plus_listeners() {
        let j = json!({
            "status": {
                "conditions": [{"type": "Accepted", "status": "True"}],
                "listeners": [
                    {"name": "http", "conditions": [
                        {"type": "Programmed", "status": "False"},
                        {"type": "Conflicted", "status": "True"}
                    ]}
                ]
            }
        });
        let conds = scan_conditions(&j);
        assert_eq!(conds.len(), 3);
        let types: Vec<&str> = conds.iter().map(|c| c.type_.as_str()).collect();
        assert!(types.contains(&"Accepted"));
        assert!(types.contains(&"Programmed"));
        assert!(types.contains(&"Conflicted"));
    }

    #[test]
    fn malformed_condition_missing_type_is_skipped() {
        let j = json!({
            "status": {
                "conditions": [
                    {"status": "False"},
                    {"type": "Accepted", "status": "False"}
                ]
            }
        });
        let conds = scan_conditions(&j);
        assert_eq!(conds.len(), 1);
        assert_eq!(conds[0].type_, "Accepted");
    }

    #[test]
    fn no_conditions_anywhere_yields_empty() {
        let j = json!({"spec": {"x": 1}, "status": {"ready": true}});
        assert!(scan_conditions(&j).is_empty());
    }

    #[test]
    fn conditions_at_top_level_object_not_array_is_ignored() {
        let j = json!({"conditions": {"not": "an array"}});
        assert!(scan_conditions(&j).is_empty());
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;

    #[test]
    fn empty_snapshot_renders_empty_string() {
        let out = render_snapshot(&[]);
        assert_eq!(out, "");
    }

    #[test]
    fn non_empty_snapshot_has_help_type_and_sorted_lines() {
        let entries = vec![
            Entry {
                kind: "HTTPRoute",
                namespace: "ns-b".into(),
                name: "route".into(),
                ty: Kind::Err,
            },
            Entry {
                kind: "HTTPRoute",
                namespace: "ns-a".into(),
                name: "route".into(),
                ty: Kind::Warn,
            },
            Entry {
                kind: "Gateway",
                namespace: "ns-z".into(),
                name: "gw".into(),
                ty: Kind::Err,
            },
        ];
        let mut sorted = entries.clone();
        sorted.sort();

        let out = render_snapshot(&sorted);
        let expected = "\
# HELP edgion_controller_resource_abnormal Resource currently has at least one False status condition. type=err for Accepted/ResolvedRefs/Programmed failures (or Conflicted/ListenersNotValid=True), type=warn for Ready:False or PartiallyInvalid:True.
# TYPE edgion_controller_resource_abnormal gauge
edgion_controller_resource_abnormal{kind=\"Gateway\",namespace=\"ns-z\",name=\"gw\",type=\"err\"} 1
edgion_controller_resource_abnormal{kind=\"HTTPRoute\",namespace=\"ns-a\",name=\"route\",type=\"warn\"} 1
edgion_controller_resource_abnormal{kind=\"HTTPRoute\",namespace=\"ns-b\",name=\"route\",type=\"err\"} 1
";
        assert_eq!(out, expected);
    }

    #[test]
    fn label_values_escape_backslash_and_quote() {
        let entries = vec![Entry {
            kind: "HTTPRoute",
            namespace: "n\"s".into(),
            name: "back\\slash".into(),
            ty: Kind::Err,
        }];
        let out = render_snapshot(&entries);
        assert!(out.contains(r#"namespace="n\"s""#));
        assert!(out.contains(r#"name="back\\slash""#));
    }
}

#[cfg(test)]
mod registration_tests {
    use super::*;
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::raw_store::{
        LocalRawStore, RawStore,
    };
    use serde::Serialize;
    use std::sync::Arc;

    #[derive(Clone, Debug, Serialize)]
    struct FakeRoute {
        metadata: FakeMeta,
        status: FakeStatus,
    }
    #[derive(Clone, Debug, Serialize)]
    struct FakeMeta {
        namespace: String,
        name: String,
    }
    #[derive(Clone, Debug, Serialize)]
    struct FakeStatus {
        conditions: Vec<FakeCond>,
    }
    #[derive(Clone, Debug, Serialize)]
    struct FakeCond {
        #[serde(rename = "type")]
        type_: String,
        status: String,
    }

    fn mk(ns: &str, name: &str, type_: &str, status: &str) -> Arc<FakeRoute> {
        Arc::new(FakeRoute {
            metadata: FakeMeta { namespace: ns.into(), name: name.into() },
            status: FakeStatus {
                conditions: vec![FakeCond { type_: type_.into(), status: status.into() }],
            },
        })
    }

    #[test]
    fn render_kind_emits_entries_only_for_abnormal_resources() {
        let store: Arc<dyn RawStore<FakeRoute>> = Arc::new({
            let s = LocalRawStore::<FakeRoute>::new();
            s.upsert("default/bad".into(), mk("default", "bad", "Accepted", "False"));
            s.upsert("default/good".into(), mk("default", "good", "Accepted", "True"));
            s.upsert("default/warn".into(), mk("default", "warn", "Ready", "False"));
            s
        });

        let mut buf: Vec<Entry> = Vec::new();
        render_kind("FakeRoute", store.as_ref(), &mut buf);

        assert_eq!(buf.len(), 2);
        let bad = buf.iter().find(|e| e.name == "bad").unwrap();
        assert_eq!(bad.ty, Kind::Err);
        assert_eq!(bad.kind, "FakeRoute");
        let warn = buf.iter().find(|e| e.name == "warn").unwrap();
        assert_eq!(warn.ty, Kind::Warn);
    }
}
