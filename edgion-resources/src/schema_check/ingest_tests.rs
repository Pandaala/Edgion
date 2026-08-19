//! Tests for the ingest-capture hook: the `strict_ingest_enabled` gate plus
//! the hand-written `Deserialize` impls on `EdgionPluginsSpec` /
//! `EdgionStreamPluginsSpec` that populate `unknown_fields`.
//!
//! The gate is process-global (`static AtomicBool`), and `cargo test` runs
//! every test in this binary concurrently, so every test that flips it MUST
//! serialize through `GATE_LOCK` and restore the prior value on the way out
//! — otherwise two tests toggling the gate in parallel would race each
//! other's assertions.
//!
//! **Invariant this hook relies on**: every gated ingress (today: the K8s
//! watcher; Phase 2b adds the Admin API write path and the FS/etcd loaders)
//! carries an OPERATOR-AUTHORED document. A controller-resolved document
//! (one carrying a populated `#[schemars(skip)]` field such as
//! `policy_action` or a plugin's `resolved_*` credential map) travels only
//! over conf_sync to the gateway, where the gate is always `false`. If a
//! future change ever feeds a resolved document into a gated seam, its
//! `#[schemars(skip)]` fields would surface as false-positive findings —
//! that is the tripwire to watch for. See `realistic_reingest_round_trip_has_zero_findings`
//! and `operator_written_policy_action_is_reported_by_design` below for why
//! this holds today.

use parking_lot::Mutex;
use serde_json::json;

use super::{set_strict_ingest, strict_ingest_enabled};
use crate::resources::edgion_plugins::EdgionPluginsSpec;
use crate::resources::edgion_stream_plugins::EdgionStreamPluginsSpec;

/// Serializes every test in this module that touches the gate.
static GATE_LOCK: Mutex<()> = Mutex::new(());

/// RAII guard: takes the lock, sets the gate, and restores the PRIOR value
/// (not unconditionally `false`) when dropped — so tests can nest cleanly
/// and never leak a changed gate value into an unrelated test outside this
/// module.
struct GateGuard<'a> {
    _lock: parking_lot::MutexGuard<'a, ()>,
    prior: bool,
}

impl<'a> GateGuard<'a> {
    fn set(lock: &'a Mutex<()>, enabled: bool) -> Self {
        let guard = lock.lock();
        let prior = strict_ingest_enabled();
        set_strict_ingest(enabled);
        GateGuard { _lock: guard, prior }
    }
}

impl Drop for GateGuard<'_> {
    fn drop(&mut self) {
        set_strict_ingest(self.prior);
    }
}

// ============================================================
// Test 1 — gate OFF: a typo'd document deserializes cleanly with
// `unknown_fields` empty (fast path, no walk performed).
// ============================================================

#[test]
fn gate_off_typo_document_yields_empty_unknown_fields() {
    let _guard = GateGuard::set(&GATE_LOCK, false);

    let doc = json!({
        "requestPlugins": [
            {"type": "IpRestriction", "config": {"defaultAction": "allow", "denys": []}}
        ]
    });

    let spec: EdgionPluginsSpec = serde_json::from_value(doc).expect("shadow path accepts the typo silently");
    assert!(spec.unknown_fields.is_empty());
}

// ============================================================
// Test 2 — gate ON: Kubernetes-shaped JSON with `config.denys` reports
// exactly `requestPlugins[0].config.denys`.
// ============================================================

#[test]
fn gate_on_reports_exact_path_for_plugin_config_typo() {
    let _guard = GateGuard::set(&GATE_LOCK, true);

    let doc = json!({
        "requestPlugins": [
            {"type": "IpRestriction", "config": {"defaultAction": "allow", "denys": []}}
        ]
    });

    let spec: EdgionPluginsSpec = serde_json::from_value(doc).expect("well-formed aside from the typo");
    assert_eq!(spec.unknown_fields, vec!["requestPlugins[0].config.denys".to_string()]);
}

// ============================================================
// Test 3 — gate ON: EdgionStreamPluginsSpec with a typo in a stream plugin
// config is reported too (R-6: the L4 resource gets the same hook).
// ============================================================

#[test]
fn gate_on_reports_typo_in_stream_plugin_config() {
    let _guard = GateGuard::set(&GATE_LOCK, true);

    let doc = json!({
        "plugins": [
            {"type": "IpRestriction", "config": {"defaultAction": "allow", "denys": []}}
        ]
    });

    let spec: EdgionStreamPluginsSpec = serde_json::from_value(doc).expect("well-formed aside from the typo");
    assert_eq!(spec.unknown_fields, vec!["plugins[0].config.denys".to_string()]);
}

// ============================================================
// Test 4a — the realistic re-ingest round trip has zero findings.
//
// Every `#[schemars(skip)]`-but-serialized field in this crate (the
// `policy_action` family and the `resolved_*` credential fields, e.g.
// `KeyAuthConfig::resolved_keys`) also carries `skip_serializing_if`, so a
// freshly-parsed, NOT-YET-RESOLVED object serializes without any of them.
// The only re-ingest that actually happens at a gated seam is: Admin API
// parses an operator document, persists `serde_json::to_string(&obj)`
// verbatim (FS/etcd), and the loader reads it back later. This models
// exactly that path — deserialize an operator document (with an auth
// plugin that HAS a `resolved_*` field, left at its natural `None`), turn
// it into a `Value`, and feed the value back through the gated path.
//
// Deliberately does NOT hand-populate `policy_action` or any `resolved_*`
// field: the architecture never produces that input at a gated ingress (see
// the module doc for the full invariant). A resolved document exists only
// on the conf_sync wire, where the gate is always off.
// ============================================================

#[test]
fn realistic_reingest_round_trip_has_zero_findings() {
    let _guard = GateGuard::set(&GATE_LOCK, true);

    let doc = json!({
        "requestPlugins": [
            {
                "type": "KeyAuth",
                "config": {
                    "keySources": [{"type": "header", "name": "X-API-Key"}],
                    "secretGroups": [
                        {"name": "default", "secretRefs": [{"name": "api-keys"}]}
                    ]
                }
            },
            {
                "type": "IpRestriction",
                "config": {"defaultAction": "allow", "deny": [{"name": "blocklist", "cidrs": ["10.0.0.0/8"]}]}
            }
        ]
    });

    // First parse: an operator-authored document, no `resolved_*` /
    // `policy_action` fields present — exactly what an Admin API write
    // parses.
    let parsed: EdgionPluginsSpec = serde_json::from_value(doc).expect("realistic operator document parses cleanly");
    assert_eq!(
        parsed.unknown_fields,
        Vec::<String>::new(),
        "sanity: the operator document itself must be well-formed"
    );

    // Second parse: what a FS/etcd loader would do after the Admin API
    // persisted `serde_json::to_string(&parsed)` — the re-ingest this test
    // exists to model.
    let value = serde_json::to_value(&parsed).expect("EdgionPluginsSpec always serializes");
    let reingested: EdgionPluginsSpec = serde_json::from_value(value).expect("a persisted document always reloads");

    assert_eq!(
        reingested.unknown_fields,
        Vec::<String>::new(),
        "an operator document that never carried controller-resolved fields must re-ingest with zero findings"
    );
}

// ============================================================
// Test 4b — an operator-written `policyAction` IS reported, by design.
//
// `policy_action` is controller-owned: `plugin_policy_stamp.rs:25`
// documents an "unconditional anti-spoofing clear" on every preparse, so a
// hand-written `policyAction:` in the submitted document is silently
// discarded regardless of what this checker does. Reporting it is exactly
// on-message for this feature — "the key you wrote has no effect" — not a
// false positive to suppress.
// ============================================================

#[test]
fn operator_written_policy_action_is_reported_by_design() {
    let _guard = GateGuard::set(&GATE_LOCK, true);

    let doc = json!({
        "requestPlugins": [
            {
                "type": "IpRestriction",
                "config": {"defaultAction": "allow"},
                "policyAction": {"action": "bypass"}
            }
        ]
    });

    let spec: EdgionPluginsSpec = serde_json::from_value(doc).expect("well-formed aside from the operator field");
    assert_eq!(
        spec.unknown_fields,
        vec!["requestPlugins[0].policyAction".to_string()],
        "an operator-supplied policyAction is controller-owned and unconditionally cleared \
         (plugin_policy_stamp.rs anti-spoofing); reporting it tells the operator the key has no effect"
    );
}

// ============================================================
// Test 5 — round-trip: `unknown_fields` never appears in
// `serde_json::to_value(&spec)` output, regardless of gate state.
// ============================================================

#[test]
fn unknown_fields_never_serialized() {
    let _guard = GateGuard::set(&GATE_LOCK, true);

    let doc = json!({
        "requestPlugins": [
            {"type": "IpRestriction", "config": {"defaultAction": "allow", "denys": []}}
        ]
    });
    let spec: EdgionPluginsSpec = serde_json::from_value(doc).expect("well-formed aside from the typo");
    assert!(
        !spec.unknown_fields.is_empty(),
        "sanity: the finding actually populated"
    );

    let out = serde_json::to_value(&spec).expect("EdgionPluginsSpec always serializes");
    assert!(
        out.get("unknownFields").is_none() && out.get("unknown_fields").is_none(),
        "unknown_fields must never ride the wire: {out}"
    );
}
