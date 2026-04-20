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
use std::sync::OnceLock;

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
