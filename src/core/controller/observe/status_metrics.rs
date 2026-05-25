//! `edgion_controller_resource_abnormal` gauge.
//!
//! See `docs/superpowers/specs/2026-04-20-controller-resource-abnormal-metrics-design.md`
//! for the full design. Short version:
//!
//! * A background task rebuilds a fresh `Vec<Entry>` every `tick_secs` by
//!   reading every registered `RawStore`, serializing each resource to JSON,
//!   recursively scanning for `"conditions": [...]` arrays, and partitioning
//!   the matching triggers into `err_conditions` / `warn_conditions`.
//! * The snapshot is published atomically via `ArcSwap<Arc<Vec<Entry>>>`.
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
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use crate::core::controller::conf_mgr::sync_runtime::resource_processor::raw_store::RawStore;

pub(crate) const METRIC_NAME: &str = "edgion_controller_resource_abnormal";
pub(crate) const METRIC_HELP: &str = "Resource currently has at least one False status condition. The `err` label is a JSON array of err-level condition triggers (Accepted/ResolvedRefs/Programmed:False, Conflicted/ListenersNotValid:True). The `warn` label is a JSON array of warn-level condition triggers (Ready:False, PartiallyInvalid:True). Empty string means no condition at that severity.";

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
}

/// Renderer closure signature: pushes `Entry` values for one kind's store.
pub(crate) type Renderer = Box<dyn Fn(&mut Vec<Entry>) + Send + Sync>;

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
pub(crate) fn renderers_slot() -> &'static Mutex<HashMap<&'static str, Renderer>> {
    static RENDERERS: OnceLock<Mutex<HashMap<&'static str, Renderer>>> = OnceLock::new();
    RENDERERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Test-only module-level mutex. All tests in this file that mutate the
/// global `snapshot_slot()` or `renderers_slot()` must lock this before
/// touching state, to serialize otherwise parallel cargo test threads
/// running in the same process.
#[cfg(test)]
static TEST_MUTEX: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

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
/// - `Conflicted:True`, `ListenersNotValid:True` (inverse semantics —
///   True is bad for these two)
///
/// Warn triggers:
/// - `Ready:False`
/// - `PartiallyInvalid:True` (inverse)
pub(crate) fn classify(conditions: &[Condition]) -> Option<(Vec<ConditionRef>, Vec<ConditionRef>)> {
    let mut err_list = Vec::new();
    let mut warn_list = Vec::new();
    for c in conditions {
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

/// Render a sorted snapshot to Prometheus text format. Returns empty
/// string on empty input so the wrapper handler can skip appending
/// entirely.
pub(crate) fn render_snapshot(entries: &[Entry]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(128 + entries.len() * 128);
    out.push_str("# HELP ");
    out.push_str(METRIC_NAME);
    out.push(' ');
    out.push_str(METRIC_HELP);
    out.push('\n');
    out.push_str("# TYPE ");
    out.push_str(METRIC_NAME);
    out.push_str(" gauge\n");
    for e in entries {
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
pub(crate) fn render_kind<K>(kind: &'static str, store: &dyn RawStore<K>, out: &mut Vec<Entry>)
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
        if let Some((err_conditions, warn_conditions)) = classify(&conds) {
            out.push(Entry {
                kind,
                namespace: ns,
                name,
                err_conditions,
                warn_conditions,
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
    let renderer: Renderer = Box::new(move |buf: &mut Vec<Entry>| {
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
    let mut buf: Vec<Entry> = Vec::new();
    {
        let guard = renderers_slot().lock();
        for renderer in guard.values() {
            renderer(&mut buf);
        }
    }
    buf.sort();

    let cap = hard_cap();
    if buf.len() > cap {
        tracing::error!(
            component = "status_metrics",
            count = buf.len(),
            cap = cap,
            "abnormal-resource snapshot exceeds hard cap; truncating"
        );
        buf.truncate(cap);
    }

    snapshot_slot().store(Arc::new(buf));
}

use axum::response::{IntoResponse, Response};
use http::{header, StatusCode};

/// Controller-side `/metrics` handler. Replaces the direct mount of
/// `common::observe::metrics_api::metrics_handler` in the admin router
/// by delegating to it first, then appending the resource-abnormal
/// block when the snapshot is non-empty.
pub(crate) async fn metrics_handler() -> Response {
    let base = crate::core::common::observe::metrics_api::metrics_handler().await;
    let (parts, body) = base.into_parts();
    if parts.status != StatusCode::OK {
        return Response::from_parts(parts, body);
    }

    let base_bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to read base metrics body").into_response();
        }
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

// Unit tests for this module live in `tests/status_metrics_test.rs`.
#[cfg(test)]
#[path = "tests/status_metrics_test.rs"]
mod tests;
