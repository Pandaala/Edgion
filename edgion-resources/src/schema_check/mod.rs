//! Walk a JSON document against a `schemars`-generated JSON Schema and report
//! the input keys the schema does not recognize.
//!
//! This exists because `serde_ignored` cannot see into an adjacently-tagged
//! enum (`#[serde(tag = "type", content = "config")]`): the tag-dispatching
//! deserializer buffers the `config` content before the tag is known, so any
//! typo inside a plugin's `config` block is silently dropped instead of
//! surfaced. Walking the schema instead of the deserializer sidesteps that
//! blind spot entirely. See
//! `skills/04-review/resources/compatibility-constraints/serde-ignored-blind-spots-schema-walk-instead.md`
//! for the measured comparison and the rules this module encodes.
//!
//! The resource-agnostic checker core is reused by the hand-written
//! `Deserialize` implementations on `EdgionPluginsSpec`,
//! `EdgionStreamPluginsSpec`, `EdgionConfigDataSpec`, and `LinkSysSpec`.
//! Controller ingress, status, diagnostics, and metric surfaces consume the
//! captured paths without changing resource acceptance.

mod walk;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod ingest_tests;

use schemars::Schema;
use serde_json::{Map, Value};

/// Report every input object key the schema does not recognize, as dotted
/// paths (array indices appended as `[i]`, e.g.
/// `requestPlugins[0].config.denys`). The result is sorted, so output is
/// deterministic across runs for identical input
/// — required both because Controller status conditions
/// must not churn on every relist, and so a
/// diff against expected output is stable in tests.
///
/// See `walk` for the traversal rules.
pub fn unknown_key_paths(value: &Value, schema: &Schema) -> Vec<String> {
    let root = schema.as_value();
    // Borrowed, not cloned: this runs once per checked resource ingest, and
    // `$defs` is the whole definitions map for the schema —
    // deep-cloning it on every call would be needless allocation on a hot
    // path. `empty` only backs the borrow when the schema has no `$defs` at
    // all (a schema with no nested `$ref` targets).
    let empty = Map::new();
    let defs: &Map<String, Value> = root.get("$defs").and_then(Value::as_object).unwrap_or(&empty);

    let mut findings = Vec::new();
    let mut node_count: u64 = 0;
    walk::walk(value, root, defs, "", 0, &mut node_count, &mut findings);
    findings.sort();
    findings
}

use std::sync::atomic::{AtomicBool, Ordering};

/// Whether the current process treats a supported resource-spec deserialize as
/// "Kubernetes watcher ingest" and should pay for the `Value` round-trip plus
/// the schema walk.
///
/// Deliberately an `AtomicBool`, not a `OnceLock<bool>`: tests toggle this
/// back and forth within one process (see the shared `Mutex` guard each such
/// test takes), which a `OnceLock` cannot support. Production code sets it
/// exactly ONCE, at controller startup, before any resource is deserialized
/// off the Kubernetes watch — see `KubernetesCenter::new` in
/// `edgion-controller/src/conf_mgr/conf_center/kubernetes/center.rs`.
///
/// Why Kubernetes-only: `serde_json::Value::deserialize`
/// CONSUMES the `Deserializer`. If the round-trip ever failed on something
/// `Value` cannot represent (a YAML non-string key, `!!binary`, a tag), the
/// resource would go from "loads fine" to "fails to load" with no fallback
/// possible — the original `D` is already gone. In Kubernetes mode the input
/// is always JSON (kube deserializes with `serde_json`), so the round-trip is
/// lossless by construction and this failure mode cannot occur. FileSystem
/// and etcd get their own check at their own loaders instead,
/// where the parsed value already exists and a failed check can simply be
/// skipped rather than aborting the load.
///
/// **Invariant every gated ingress relies on**: the document reaching a
/// gated `Deserialize` call is always OPERATOR-AUTHORED — never a
/// controller-resolved object. A resolved object (one carrying a populated
/// `#[schemars(skip)]` field, e.g. a plugin's
/// `resolved_*` credential map) travels only over conf_sync to the gateway,
/// where this flag is always `false`. Should a future change ever feed a
/// resolved object into a gated seam, its skip-only fields would surface as
/// false-positive findings — see `schema_check::ingest_tests` for the tests
/// pinning this invariant and its resolved-field boundaries.
static STRICT_INGEST: AtomicBool = AtomicBool::new(false);

/// See [`STRICT_INGEST`] for what this flag means and why it exists only for
/// the Kubernetes ingest path.
pub fn strict_ingest_enabled() -> bool {
    STRICT_INGEST.load(Ordering::Relaxed)
}

/// Set the [`STRICT_INGEST`] flag. Production calls this exactly once, at
/// Kubernetes `ConfCenter` construction, before any resource is watched.
/// Tests may call it repeatedly but MUST serialize against a shared `Mutex`
/// guard and restore the prior value on the way out, since this is
/// process-global state and `cargo test` runs tests in one binary
/// concurrently.
pub fn set_strict_ingest(enabled: bool) {
    STRICT_INGEST.store(enabled, Ordering::Relaxed);
}
