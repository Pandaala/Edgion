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

#[cfg(test)]
mod classify_tests {
    use super::*;

    fn c(t: &str, s: &str) -> Condition {
        Condition {
            type_: t.into(),
            status: s.into(),
        }
    }

    #[test]
    fn err_and_warn_coexist_both_preserved() {
        let cs = vec![c("Accepted", "False"), c("Ready", "False")];
        let (err, warn) = classify(&cs).unwrap();
        assert_eq!(err.len(), 1);
        assert_eq!(err[0].type_, "Accepted");
        assert_eq!(warn.len(), 1);
        assert_eq!(warn[0].type_, "Ready");
    }

    #[test]
    fn unknown_type_is_ignored() {
        let cs = vec![c("SomethingElse", "False"), c("Accepted", "True")];
        assert!(classify(&cs).is_none());
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

#[cfg(test)]
mod scan_conditions_tests {
    use super::*;
    use serde_json::json;

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
}

#[cfg(test)]
mod render_tests {
    use super::*;

    #[test]
    fn label_values_escape_backslash_and_quote() {
        let entries = vec![Entry {
            kind: "HTTPRoute",
            namespace: "n\"s".into(),
            name: "back\\slash".into(),
            err_conditions: vec![ConditionRef {
                type_: "Accepted".into(),
                status: "False".into(),
            }],
            warn_conditions: vec![],
        }];
        let out = render_snapshot(&entries);
        assert!(out.contains(r#"namespace="n\"s""#));
        assert!(out.contains(r#"name="back\\slash""#));
    }
}

#[cfg(test)]
mod registration_tests {
    use super::*;
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::raw_store::{LocalRawStore, RawStore};
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
            metadata: FakeMeta {
                namespace: ns.into(),
                name: name.into(),
            },
            status: FakeStatus {
                conditions: vec![FakeCond {
                    type_: type_.into(),
                    status: status.into(),
                }],
            },
        })
    }

    #[test]
    fn render_kind_emits_entries_only_for_abnormal_resources() {
        let _guard = super::TEST_MUTEX.lock();
        renderers_slot().lock().clear();
        snapshot_slot().store(Arc::new(Vec::new()));
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
        assert_eq!(bad.err_conditions.len(), 1);
        assert_eq!(bad.err_conditions[0].type_, "Accepted");
        assert!(bad.warn_conditions.is_empty());
        assert_eq!(bad.kind, "FakeRoute");
        let warn = buf.iter().find(|e| e.name == "warn").unwrap();
        assert!(warn.err_conditions.is_empty());
        assert_eq!(warn.warn_conditions.len(), 1);
        assert_eq!(warn.warn_conditions[0].type_, "Ready");
    }
}

#[cfg(test)]
mod snapshot_loop_tests {
    use super::*;
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::raw_store::LocalRawStore;
    use serde::Serialize;
    use std::sync::Arc;

    #[derive(Clone, Serialize)]
    struct Dummy {
        metadata: Meta,
        status: St,
    }
    #[derive(Clone, Serialize)]
    struct Meta {
        namespace: String,
        name: String,
    }
    #[derive(Clone, Serialize)]
    struct St {
        conditions: Vec<Cond>,
    }
    #[derive(Clone, Serialize)]
    struct Cond {
        #[serde(rename = "type")]
        type_: String,
        status: String,
    }

    fn reset() {
        let mut g = renderers_slot().lock();
        g.clear();
        snapshot_slot().store(Arc::new(Vec::new()));
        // Restore default hard cap in case a previous test changed it.
        set_hard_cap(1000);
    }

    fn new_dummy(ns: &str, name: &str, type_: &str, status: &str) -> Arc<Dummy> {
        Arc::new(Dummy {
            metadata: Meta {
                namespace: ns.into(),
                name: name.into(),
            },
            status: St {
                conditions: vec![Cond {
                    type_: type_.into(),
                    status: status.into(),
                }],
            },
        })
    }

    /// Guards against the conf_center-reload duplication bug: a second
    /// `register_status_renderer` call for the same kind must replace the
    /// prior renderer (and drop its Arc to the old RawStore), NOT append.
    /// Otherwise every reload would double the number of entries each kind
    /// contributes per tick.
    #[test]
    fn re_registering_same_kind_replaces_old_renderer() {
        let _guard = super::TEST_MUTEX.lock();
        reset();

        // First registration: one abnormal resource named "old".
        let old_store = Arc::new(LocalRawStore::<Dummy>::new());
        old_store.upsert("a/old".into(), new_dummy("a", "old", "Accepted", "False"));
        register_status_renderer::<Dummy>("Dummy", old_store.clone());

        // Simulated reload: second registration for the SAME kind with a
        // freshly-constructed store exposing a differently-named resource.
        let new_store = Arc::new(LocalRawStore::<Dummy>::new());
        new_store.upsert("a/new".into(), new_dummy("a", "new", "Accepted", "False"));
        register_status_renderer::<Dummy>("Dummy", new_store.clone());

        snapshot_once();
        let snap = snapshot_slot().load();
        // If the bug came back, len would be 2 (one entry per store)
        // AND both "old" and "new" would appear.
        assert_eq!(
            snap.len(),
            1,
            "expected exactly 1 entry (old renderer replaced); got {} entries = {:?}",
            snap.len(),
            snap.iter().map(|e| e.name.clone()).collect::<Vec<_>>()
        );
        assert_eq!(snap[0].name, "new");
    }

    #[test]
    fn snapshot_once_propagates_renderer_panic_but_outer_loop_isolates_it() {
        let _guard = super::TEST_MUTEX.lock();
        reset();
        // Register a panicking renderer.
        renderers_slot().lock().insert(
            "PanickingFake",
            Box::new(|_buf: &mut Vec<Entry>| {
                panic!("synthetic panic");
            }),
        );
        // New contract: panics propagate out of snapshot_once; the outer
        // `spawn_snapshot_loop` task wraps each call in catch_unwind so
        // the background loop survives.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(snapshot_once));
        assert!(result.is_err(), "panic should propagate out of snapshot_once");
    }
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

#[cfg(test)]
mod perf_tests {
    use super::*;
    use crate::core::controller::conf_mgr::sync_runtime::resource_processor::raw_store::LocalRawStore;
    use serde::Serialize;
    use std::sync::Arc;
    use std::time::Instant;

    #[derive(Clone, Serialize)]
    struct Mock {
        metadata: M,
        status: S,
    }
    #[derive(Clone, Serialize)]
    struct M {
        namespace: String,
        name: String,
    }
    #[derive(Clone, Serialize)]
    struct S {
        parents: Vec<P>,
    }
    #[derive(Clone, Serialize)]
    struct P {
        conditions: Vec<C>,
    }
    #[derive(Clone, Serialize)]
    struct C {
        #[serde(rename = "type")]
        type_: String,
        status: String,
    }

    fn reset() {
        renderers_slot().lock().clear();
        snapshot_slot().store(Arc::new(Vec::new()));
        set_hard_cap(1000);
    }

    #[test]
    fn snapshot_1000_routes_under_500ms() {
        let _guard = super::TEST_MUTEX.lock();
        reset();
        let store = Arc::new(LocalRawStore::<Mock>::new());
        for i in 0..1000 {
            let conds = vec![
                C {
                    type_: "Accepted".into(),
                    status: "True".into(),
                },
                C {
                    type_: "ResolvedRefs".into(),
                    status: if i % 5 == 0 { "False".into() } else { "True".into() },
                },
            ];
            let obj = Arc::new(Mock {
                metadata: M {
                    namespace: "ns".into(),
                    name: format!("r{}", i),
                },
                status: S {
                    parents: vec![P { conditions: conds }],
                },
            });
            store.upsert(format!("ns/r{}", i), obj);
        }
        register_status_renderer::<Mock>("Mock", store);

        let t = Instant::now();
        snapshot_once();
        let elapsed = Instant::now().saturating_duration_since(t);
        assert!(
            elapsed.as_millis() < 500,
            "snapshot_once with 1000 routes took {}ms (limit 500ms) — scan_conditions may have regressed",
            elapsed.as_millis()
        );

        let snap = snapshot_slot().load();
        assert_eq!(snap.len(), 200);
    }
}
