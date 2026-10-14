//! `edgion_controller_resource_abnormal` gauge.
//!
//! See `docs/zh-CN/dev-guide/resource-abnormal-metric.md`
//! for the full design. Short version:
//!
//! * A background task rebuilds a bounded, sorted snapshot every `tick_secs` by
//!   reading every registered `RawStore`, serializing each resource to JSON,
//!   recursively scanning for `"conditions": [...]` arrays, and partitioning
//!   the matching triggers into `err_conditions` / `warn_conditions`.
//! * The snapshot is published atomically via `ArcSwap<Vec<Entry>>`.
//! * `/metrics` is read-only; it loads the current snapshot and appends
//!   a text block to the common Prometheus handler output.
//! * No incremental state, no per-resource hooks — every tick fully
//!   replaces the Vec, so recovery and deletion are automatic.
//!
//! Cardinality: one series per abnormal resource at most. Hard safety cap
//! defaults to 1000 entries per snapshot and is configurable via
//! [`set_hard_cap`] (see `ResourceAbnormalMetricsConfig::hard_cap`).

use arc_swap::ArcSwap;
use parking_lot::Mutex;
use std::collections::{BinaryHeap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use crate::conf_mgr::sync_runtime::resource_processor::raw_store::RawStore;

pub(crate) const METRIC_NAME: &str = "edgion_controller_resource_abnormal";
pub(crate) const METRIC_HELP: &str = "Resource currently has at least one False status condition. The `err` label is a JSON array of err-level condition triggers (Accepted/ResolvedRefs/Programmed:False, Conflicted/ListenersNotValid:True). The `warn` label is a JSON array of warn-level condition triggers (Ready:False, PartiallyInvalid:True). Empty string means no condition at that severity.";

pub(crate) const COND_COUNT_METRIC_NAME: &str = "edgion_controller_status_conditions_max";
pub(crate) const COND_COUNT_METRIC_HELP: &str = "Largest single .status.conditions array length on a resource. Emitted only when it reaches the configured threshold (default 20), so healthy fleets emit nothing. A bounded conditions array that keeps growing is a status-write loop (single-controller death-loop) or cross-controller condition pollution; the value is the array length.";

/// Runtime-configurable safety ceiling: if a snapshot would exceed this,
/// it is truncated and a single `tracing::error!` is emitted. Default 1000,
/// settable via [`set_hard_cap`] from the CLI startup path.
static HARD_CAP: AtomicUsize = AtomicUsize::new(1000);

/// Install the configured hard cap. Called once from controller CLI
/// startup before [`spawn_snapshot_loop`].
pub(crate) fn set_hard_cap(cap: usize) {
    HARD_CAP.store(cap, Ordering::Relaxed);
}

#[inline]
fn hard_cap() -> usize {
    HARD_CAP.load(Ordering::Relaxed)
}

/// Threshold (largest single conditions-array length) at or above which a
/// resource is emitted as an `edgion_controller_status_conditions_max` series.
/// Default 20, settable via [`set_status_conditions_threshold`] from CLI startup.
static COND_COUNT_THRESHOLD: AtomicUsize = AtomicUsize::new(20);

/// Install the configured conditions-array threshold. Called once from
/// controller CLI startup before [`spawn_snapshot_loop`].
pub(crate) fn set_status_conditions_threshold(threshold: usize) {
    COND_COUNT_THRESHOLD.store(threshold, Ordering::Relaxed);
}

#[inline]
fn cond_count_threshold() -> usize {
    COND_COUNT_THRESHOLD.load(Ordering::Relaxed)
}

/// One condition trigger referenced in the `err` / `warn` label payload.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub(crate) struct ConditionRef {
    #[serde(rename = "type")]
    pub type_: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Entry {
    /// K8s `.kind` field (PascalCase singular), e.g. `HTTPRoute`.
    pub kind: &'static str,
    pub namespace: String,
    pub name: String,
    pub err_conditions: Vec<ConditionRef>,
    pub warn_conditions: Vec<ConditionRef>,
    /// Largest single `conditions` array length seen anywhere in this
    /// resource (0 when it has no conditions). Drives the
    /// `edgion_controller_status_conditions_max` sentinel. Kept last so it is
    /// the lowest-priority `Ord` key and does not perturb existing ordering.
    pub max_conditions_len: usize,
}

/// Retain the first `cap` entries in full Entry order while counting all matches.
/// This bounds retained entries, not individual payload sizes or RawStore snapshots.
struct BoundedEntries {
    cap: usize,
    total: usize,
    items: BinaryHeap<Entry>,
}

impl BoundedEntries {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            total: 0,
            items: BinaryHeap::new(),
        }
    }

    fn push(&mut self, entry: Entry) {
        self.total += 1;
        if self.items.len() < self.cap {
            self.items.push(entry);
        } else if let Some(mut largest) = self.items.peek_mut() {
            if entry < *largest {
                *largest = entry;
            }
        }
    }

    fn into_sorted_vec(self) -> Vec<Entry> {
        self.items.into_sorted_vec()
    }
}

/// Renderer closure signature: submits entries to the shared retention bound.
type Renderer = Box<dyn Fn(&mut BoundedEntries) + Send + Sync>;

/// Global current snapshot. Read lock-free on every `/metrics` scrape;
/// replaced atomically at the end of each background tick.
pub(crate) fn snapshot_slot() -> &'static ArcSwap<Vec<Entry>> {
    static SNAPSHOT: OnceLock<ArcSwap<Vec<Entry>>> = OnceLock::new();
    SNAPSHOT.get_or_init(|| ArcSwap::from_pointee(Vec::new()))
}

/// Registered per-kind renderers, keyed by kind name so that a second
/// registration for the same kind replaces (rather than appends to) the
/// first. This is what makes the module safe across `conf_center` reloads
/// that reconstruct `FileSystemController` / `KubernetesController` /
/// `EtcdController` — each reload re-enters `register_status_renderer`
/// with a fresh `Arc<RawStore<K>>`, and the old closure is dropped,
/// releasing its reference to the previous store.
fn renderers_slot() -> &'static Mutex<HashMap<&'static str, Renderer>> {
    static RENDERERS: OnceLock<Mutex<HashMap<&'static str, Renderer>>> = OnceLock::new();
    RENDERERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A condition as observed from serialized resource JSON. Only the fields
/// we actually classify on.
#[derive(Debug, Clone)]
pub(crate) struct Condition {
    pub type_: String,
    pub status: String,
}

/// Classify a resource's full set of conditions into (err_list, warn_list).
///
/// Returns `None` if the resource is all-clean (both lists would be empty).
/// Otherwise returns `Some((err_list, warn_list))` where either list may be
/// empty but not both — the two severities coexist on the same `Entry`
/// rather than swallowing each other.
///
/// Err triggers:
/// - `Accepted:False`, `ResolvedRefs:False`, `Programmed:False`
/// - `Conflicted:True`, `ListenersNotValid:True`
///   (inverse semantics — True is bad for these two)
///
/// Warn triggers:
/// - `Ready:False`
/// - `PartiallyInvalid:True` (inverse)
pub(crate) fn classify(conditions: &[Condition]) -> Option<(Vec<ConditionRef>, Vec<ConditionRef>)> {
    let mut err_list = Vec::new();
    let mut warn_list = Vec::new();
    for c in conditions {
        // NOTE: `UnknownConfigFields` is deliberately ABSENT from this match,
        // in both severities. It
        // reports an operator typo in a document that still syncs to the
        // data plane unchanged — not resource ill-health — so it must not
        // move the `edgion_controller_resource_abnormal` gauge. The `_ => {}` arm already
        // ignores it correctly; do not "complete" this match by adding an
        // arm for it. See
        // `status_metrics_test.rs::classify_tests::unknown_config_fields_true_produces_no_series`
        // for the pin.
        match (c.type_.as_str(), c.status.as_str()) {
            ("Accepted", "False")
            | ("ResolvedRefs", "False")
            | ("Programmed", "False")
            | ("Conflicted", "True")
            | ("ListenersNotValid", "True") => err_list.push(ConditionRef {
                type_: c.type_.clone(),
                status: c.status.clone(),
            }),
            ("Ready", "False") | ("PartiallyInvalid", "True") => warn_list.push(ConditionRef {
                type_: c.type_.clone(),
                status: c.status.clone(),
            }),
            _ => {}
        }
    }
    if err_list.is_empty() && warn_list.is_empty() {
        None
    } else {
        Some((err_list, warn_list))
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
pub(crate) fn scan_conditions(value: &serde_json::Value) -> Vec<Condition> {
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

/// Largest length of any single `"conditions": [ ... ]` array anywhere in the
/// resource JSON (resource-level, per-parent, per-listener, ...). Distinct from
/// [`scan_conditions`], which flattens every array together: a status-write
/// loop appends to *one* array, so its length — not the flattened total — is
/// the loop signal (and it avoids false positives on a legitimately multi-parent
/// / multi-listener resource whose arrays are individually small).
pub(crate) fn max_conditions_array_len(value: &serde_json::Value) -> usize {
    let mut max = 0usize;
    walk_max(value, &mut max);
    max
}

fn walk_max(value: &serde_json::Value, max: &mut usize) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                if k == "conditions" {
                    if let serde_json::Value::Array(arr) = v {
                        if arr.len() > *max {
                            *max = arr.len();
                        }
                    }
                } else {
                    walk_max(v, max);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                walk_max(item, max);
            }
        }
        _ => {}
    }
}

/// Render a sorted snapshot to Prometheus text format. Returns empty
/// string on empty input so the wrapper handler can skip appending
/// entirely.
pub(crate) fn render_snapshot(entries: &[Entry]) -> String {
    // A snapshot Entry may be present only because its conditions array is
    // bloated (rendered by `render_conditions_max`), with no err/warn triggers.
    // Such an entry must NOT emit a `resource_abnormal` series.
    let abnormal: Vec<&Entry> = entries
        .iter()
        .filter(|e| !e.err_conditions.is_empty() || !e.warn_conditions.is_empty())
        .collect();
    if abnormal.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(128 + abnormal.len() * 128);
    out.push_str("# HELP ");
    out.push_str(METRIC_NAME);
    out.push(' ');
    out.push_str(METRIC_HELP);
    out.push('\n');
    out.push_str("# TYPE ");
    out.push_str(METRIC_NAME);
    out.push_str(" gauge\n");
    for e in abnormal {
        let err_json = serialize_conditions_for_label(e, &e.err_conditions, "err");
        let warn_json = serialize_conditions_for_label(e, &e.warn_conditions, "warn");
        out.push_str(METRIC_NAME);
        out.push_str("{kind=\"");
        out.push_str(e.kind);
        out.push_str("\",namespace=\"");
        append_escaped(&mut out, &e.namespace);
        out.push_str("\",name=\"");
        append_escaped(&mut out, &e.name);
        out.push_str("\",err=\"");
        append_escaped(&mut out, &err_json);
        out.push_str("\",warn=\"");
        append_escaped(&mut out, &warn_json);
        out.push_str("\"} 1\n");
    }
    out
}

/// Render the `edgion_controller_status_conditions_max` block for every entry
/// whose largest conditions array reached the threshold. Returns "" when none
/// qualify (so the wrapper appends nothing and no bare HELP/TYPE is emitted).
/// The gauge value is the array length, so an operator sees how bloated it is.
pub(crate) fn render_conditions_max(entries: &[Entry]) -> String {
    let threshold = cond_count_threshold();
    let hot: Vec<&Entry> = entries.iter().filter(|e| e.max_conditions_len >= threshold).collect();
    if hot.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(128 + hot.len() * 96);
    out.push_str("# HELP ");
    out.push_str(COND_COUNT_METRIC_NAME);
    out.push(' ');
    out.push_str(COND_COUNT_METRIC_HELP);
    out.push('\n');
    out.push_str("# TYPE ");
    out.push_str(COND_COUNT_METRIC_NAME);
    out.push_str(" gauge\n");
    for e in hot {
        out.push_str(COND_COUNT_METRIC_NAME);
        out.push_str("{kind=\"");
        out.push_str(e.kind);
        out.push_str("\",namespace=\"");
        append_escaped(&mut out, &e.namespace);
        out.push_str("\",name=\"");
        append_escaped(&mut out, &e.name);
        out.push_str("\"} ");
        out.push_str(&e.max_conditions_len.to_string());
        out.push('\n');
    }
    out
}

/// Serialize one severity bucket to its JSON-array label value. Empty
/// bucket → empty string (so PromQL `{err!=""}` can distinguish "no
/// err conditions" from "has err conditions"). Serialization failure is
/// logged at WARN and also yields an empty string — this path is not
/// theoretically reachable for our struct shape, but we log instead of
/// silently swallowing so the op has a breadcrumb if it ever trips.
fn serialize_conditions_for_label(entry: &Entry, conditions: &[ConditionRef], bucket: &'static str) -> String {
    if conditions.is_empty() {
        return String::new();
    }
    match serde_json::to_string(conditions) {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(
                component = "status_metrics",
                kind = entry.kind,
                namespace = %entry.namespace,
                name = %entry.name,
                bucket = bucket,
                error = %err,
                "failed to serialize condition bucket; emitting empty label value"
            );
            String::new()
        }
    }
}

/// Escape `\` and `"` per Prometheus text-format conventions.
///
/// `pub(crate)` so `observe::unknown_fields_registry`'s metric renderer can
/// reuse the exact same escaping rather than re-implementing it — a raw `"`
/// or `\` inside a finding path would otherwise corrupt the exposition
/// format.
pub(crate) fn append_escaped(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
}

/// Scan one kind's store and push every abnormal OR conditions-bloated resource
/// into `out`.
///
/// Expects K to serialize to JSON with at least:
/// - `.metadata.namespace` (string; "" for cluster-scoped resources)
/// - `.metadata.name` (string)
/// - `.status.conditions[]` or nested variants (handled by
///   [`scan_conditions`] / [`max_conditions_array_len`])
///
/// Any resource that fails to serialize is skipped with a single
/// `tracing::warn!`; we do not propagate the failure.
fn render_kind<K>(kind: &'static str, store: &dyn RawStore<K>, out: &mut BoundedEntries)
where
    K: serde::Serialize,
{
    let threshold = cond_count_threshold();
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
        let max_conditions_len = max_conditions_array_len(&json);
        let classified = classify(&conds);
        // Emit an Entry when the resource is abnormal (has err/warn conditions)
        // OR when its largest conditions array is bloated past the threshold —
        // a bloated array is often all-`True`/clean, so `classify` alone would
        // miss it. Both predicates keep the series set bounded to anomalies.
        let bloated = max_conditions_len >= threshold;
        if classified.is_some() || bloated {
            let (err_conditions, warn_conditions) = classified.unwrap_or_default();
            out.push(Entry {
                kind,
                namespace: ns,
                name,
                err_conditions,
                warn_conditions,
                max_conditions_len,
            });
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
pub(crate) fn register_status_renderer<K>(kind: &'static str, store: Arc<dyn RawStore<K>>)
where
    K: serde::Serialize + Send + Sync + 'static,
{
    let renderer: Renderer = Box::new(move |buf: &mut BoundedEntries| {
        render_kind(kind, store.as_ref(), buf);
    });
    // Keyed by `kind`: a reload-time second call for the same kind drops
    // the prior closure (and with it the Arc to the old RawStore), so
    // we don't accumulate duplicate renderers nor pin dead stores.
    renderers_slot().lock().insert(kind, renderer);
}

use std::time::Duration;

/// Build one fresh snapshot and atomically publish it. Exposed so tests
/// can drive a tick synchronously.
///
/// Panics from registered renderers propagate out of this function. The
/// background loop in [`spawn_snapshot_loop`] wraps each call in
/// `catch_unwind` so a single panicking renderer cannot kill the loop.
pub(crate) fn snapshot_once() {
    let cap = hard_cap();
    let mut buf = BoundedEntries::new(cap);
    {
        let guard = renderers_slot().lock();
        for renderer in guard.values() {
            renderer(&mut buf);
        }
    }
    if buf.total > cap {
        tracing::error!(
            component = "status_metrics",
            count = buf.total,
            cap = cap,
            "abnormal-resource snapshot exceeds hard cap; truncating"
        );
    }

    snapshot_slot().store(Arc::new(buf.into_sorted_vec()));
}

use axum::response::{IntoResponse, Response};
use http::{header, StatusCode};

/// Controller-side `/metrics` handler. Replaces the direct mount of
/// `common::observe::metrics_api::metrics_handler` in the admin router
/// by delegating to it first, then appending the resource-abnormal
/// block when the snapshot is non-empty.
pub(crate) async fn metrics_handler() -> Response {
    crate::observe::unknown_fields_registry::sample_unknown_config_fields_metric();
    let base = edgion_common::observe::metrics_api::metrics_handler().await;
    let (parts, body) = base.into_parts();
    if parts.status != StatusCode::OK {
        return Response::from_parts(parts, body);
    }

    let Ok(base_bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "failed to read base metrics body").into_response();
    };
    let mut text = String::from_utf8(base_bytes.to_vec()).unwrap_or_default();

    let snap = snapshot_slot().load();
    let block = render_snapshot(snap.as_ref());
    if !block.is_empty() {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&block);
    }
    let cond_block = render_conditions_max(snap.as_ref());
    if !cond_block.is_empty() {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&cond_block);
    }

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        text,
    )
        .into_response()
}

/// Extract a displayable message from a `catch_unwind` panic payload.
/// Handles the two common panic types (`&'static str` via `panic!("lit")`
/// and `String` via `panic!("{}", e)`); any other payload type yields a
/// generic placeholder so we at least log something.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload type".to_string()
    }
}

/// Spawn the background snapshot loop. Called once from controller CLI
/// startup when `config.metrics.resource_abnormal.enabled` is true.
///
/// Each tick wraps `snapshot_once` in `catch_unwind` so a renderer that
/// panics cannot kill the loop. Caught panics are logged at WARN level
/// so operators can see "snapshot tick failed" events in the Controller
/// logs (diagnosing a missing series on /metrics otherwise would require
/// guessing). After logging, we retain the previous snapshot and retry
/// on the next tick.
pub(crate) fn spawn_snapshot_loop(tick_secs: u64) {
    let tick = Duration::from_secs(tick_secs);
    tokio::spawn(async move {
        if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(snapshot_once)) {
            tracing::warn!(
                component = "status_metrics",
                error = %panic_message(&*payload),
                "initial snapshot_once panicked; keeping empty snapshot, will retry next tick"
            );
        }
        loop {
            tokio::time::sleep(tick).await;
            if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(snapshot_once)) {
                tracing::warn!(
                    component = "status_metrics",
                    error = %panic_message(&*payload),
                    "snapshot_once panicked during tick; keeping previous snapshot, will retry next tick"
                );
            }
        }
    });

    tracing::info!(
        component = "status_metrics",
        tick_secs = tick_secs,
        "resource-abnormal snapshot loop started"
    );
}

// Unit tests for this module live in `tests/unit/observe/status_metrics_test.rs`.
#[cfg(test)]
#[path = "../../tests/unit/observe/status_metrics_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/support/observe/status_metrics.rs"]
mod test_support;
#[cfg(test)]
use test_support::*;
