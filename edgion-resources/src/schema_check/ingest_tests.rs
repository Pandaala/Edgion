//! Tests for the ingest-capture hook: the `strict_ingest_enabled` gate plus
//! the hand-written `Deserialize` impls on `EdgionPluginsSpec`,
//! `EdgionStreamPluginsSpec`, `EdgionConfigDataSpec`, and `LinkSysSpec` that
//! populate `unknown_fields`.
//!
//! The gate is process-global (`static AtomicBool`), and `cargo test` runs
//! every test in this binary concurrently, so every test that flips it MUST
//! serialize through `GATE_LOCK` and restore the prior value on the way out
//! — otherwise two tests toggling the gate in parallel would race each
//! other's assertions.
//!
//! **Invariant this hook relies on**: every gated ingress (the K8s watcher)
//! carries an OPERATOR-AUTHORED document. Admin API writes and FS/etcd loaders
//! run the same checker at their raw-document seams without using this gate.
//! A controller-resolved document
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
use crate::resources::edgion_config_data::EdgionConfigDataSpec;
use crate::resources::edgion_plugins::edgion_plugins_spec_schema;
use crate::resources::edgion_plugins::EdgionPluginsSpec;
use crate::resources::edgion_stream_plugins::EdgionStreamPluginsSpec;
use crate::resources::link_sys::{link_sys_spec_schema, LinkSysSpec, SystemConfig};

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

#[test]
fn waf_schema_walk_reports_nested_unknown_field() {
    let doc = json!({
        "waf": {
            "rules": "SecAction \"id:1,phase:1,pass\"",
            "unexpected": true
        }
    });

    assert_eq!(
        super::unknown_key_paths(&doc, edgion_plugins_spec_schema()),
        vec!["waf.unexpected".to_string()]
    );
}

// ============================================================
// Test 3 — gate ON: EdgionStreamPluginsSpec with a typo in a stream plugin
// config is reported too, proving the L4 resource gets the same hook.
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

#[test]
fn config_data_gate_off_keeps_fast_path_empty() {
    let _guard = GateGuard::set(&GATE_LOCK, false);
    let spec: EdgionConfigDataSpec = serde_json::from_value(json!({
        "data": {"type": "IpList", "config": {"items": [{"name": "g", "cidrs": [], "cidrz": []}]}}
    }))
    .expect("typed serde ignores the misspelled payload field");
    assert!(spec.unknown_fields.is_empty());
}

#[test]
fn config_data_gate_on_captures_typed_typo_and_skips_serialization() {
    let _guard = GateGuard::set(&GATE_LOCK, true);
    let spec: EdgionConfigDataSpec = serde_json::from_value(json!({
        "data": {"type": "IpList", "config": {"items": [{"name": "g", "cidrs": [], "cidrz": []}]}}
    }))
    .expect("well-formed aside from the ignored field");
    assert_eq!(spec.unknown_fields, vec!["data.config.items[0].cidrz"]);

    let serialized = serde_json::to_value(&spec).expect("spec serializes");
    assert!(serialized.get("unknownFields").is_none());
    assert!(serialized.get("unknown_fields").is_none());
}

#[test]
fn config_data_url_allow_list_typo_is_advisory_with_gate_off_and_on() {
    let cases = [
        (
            json!({
                "data": {"type": "RequestAccessUrlAllowList", "config": {"items": [], "itemz": []}}
            }),
            "data.config.itemz",
        ),
        (
            json!({
                "data": {"type": "RequestAccessUrlAllowList", "config": {"items": [{
                    "name": "health", "hosts": ["api.example.com"],
                    "paths": [{"type": "Exact", "value": "/health", "valuez": "/ignored"}],
                    "conditions": {"allOf": []}
                }]}}
            }),
            "data.config.items[0].paths[0].valuez",
        ),
        (
            json!({
                "data": {"type": "RequestAccessUrlAllowList", "config": {"items": [{
                    "name": "health", "hosts": ["api.example.com"],
                    "paths": [{"type": "Exact", "value": "/health"}],
                    "conditions": {"allOf": [{"type": "directPeerIpMatch", "ips": ["192.0.2.0/24"], "ipsz": []}]}
                }]}}
            }),
            "data.config.items[0].conditions.allOf[0].ipsz",
        ),
    ];

    for (doc, expected) in cases {
        {
            let _guard = GateGuard::set(&GATE_LOCK, false);
            let spec: EdgionConfigDataSpec =
                serde_json::from_value(doc.clone()).expect("gate-off typed ConfigData must remain deserializable");
            assert!(spec.unknown_fields.is_empty());
        }

        let _guard = GateGuard::set(&GATE_LOCK, true);
        let spec: EdgionConfigDataSpec =
            serde_json::from_value(doc).expect("gate-on typed ConfigData must report rather than reject the typo");
        assert_eq!(spec.unknown_fields, [expected]);
    }
}

#[test]
fn config_data_url_allow_list_rejects_unknown_candidate_container_fields() {
    for extra in [json!({"hostz": []}), json!({"allOf": []})] {
        let mut candidate = json!({
            "name": "health",
            "hosts": ["api.example.com"],
            "paths": [{"type": "Exact", "value": "/health"}]
        });
        candidate
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let doc = json!({
            "data": {"type": "RequestAccessUrlAllowList", "config": {"items": [candidate]}}
        });
        assert!(serde_json::from_value::<EdgionConfigDataSpec>(doc).is_err());
    }
}

#[test]
fn config_data_url_allow_list_keeps_secret_match_strict() {
    let doc = json!({
        "data": {"type": "RequestAccessUrlAllowList", "config": {"items": [{
            "name": "health", "hosts": ["api.example.com"],
            "paths": [{"type": "Exact", "value": "/health"}],
            "conditions": {"allOf": [{
                "type": "secretMatch",
                "key": {"type": "header", "name": "X-Access-Key"},
                "secretKey": "token",
                "secretRef": {"name": "access-secret"},
                "unexpectedCredentialField": true
            }]}
        }]}}
    });

    for gate in [false, true] {
        let _guard = GateGuard::set(&GATE_LOCK, gate);
        assert!(
            serde_json::from_value::<EdgionConfigDataSpec>(doc.clone()).is_err(),
            "credential/auth sub-structures must remain strict with gate={gate}"
        );
    }
}

#[test]
fn config_data_misc_is_open_under_gated_ingest() {
    let _guard = GateGuard::set(&GATE_LOCK, true);
    let spec: EdgionConfigDataSpec = serde_json::from_value(json!({
        "data": {"type": "Misc", "config": {"vendor": {"arbitrary": [1, 2, 3]}}}
    }))
    .expect("Misc accepts arbitrary JSON");
    assert!(spec.unknown_fields.is_empty());
}

#[test]
fn link_sys_gate_off_and_on_preserve_advisory_semantics() {
    let doc = json!({
        "type": "redis",
        "config": {"endpoints": ["redis://127.0.0.1:6379"], "timeout": {"conect": 10}}
    });

    {
        let _guard = GateGuard::set(&GATE_LOCK, false);
        let spec: LinkSysSpec = serde_json::from_value(doc.clone()).expect("gate-off LinkSys parses");
        assert!(spec.unknown_fields.is_empty());
    }

    let _guard = GateGuard::set(&GATE_LOCK, true);
    let spec: LinkSysSpec = serde_json::from_value(doc).expect("gate-on LinkSys remains accepted");
    assert_eq!(spec.unknown_fields, ["config.timeout.conect"]);
    let serialized = serde_json::to_value(&spec).expect("LinkSysSpec serializes");
    assert!(serialized.get("unknownFields").is_none());
    assert!(serialized.get("unknown_fields").is_none());
    fn has_property(value: &serde_json::Value, property: &str) -> bool {
        match value {
            serde_json::Value::Object(object) => {
                object
                    .get("properties")
                    .and_then(serde_json::Value::as_object)
                    .is_some_and(|properties| properties.contains_key(property))
                    || object.values().any(|child| has_property(child, property))
            }
            serde_json::Value::Array(array) => array.iter().any(|child| has_property(child, property)),
            _ => false,
        }
    }
    assert!(!has_property(link_sys_spec_schema().as_value(), "unknown_fields"));
    assert!(!has_property(link_sys_spec_schema().as_value(), "unknownFields"));
}

#[test]
fn link_sys_strict_credential_boundary_is_unchanged_by_gate() {
    let doc = json!({
        "type": "redis",
        "config": {
            "endpoints": ["redis://127.0.0.1:6379"],
            "auth": {"secretRef": {"name": "redis-creds"}, "bogus": true}
        }
    });

    for enabled in [false, true] {
        let _guard = GateGuard::set(&GATE_LOCK, enabled);
        assert!(
            serde_json::from_value::<LinkSysSpec>(doc.clone()).is_err(),
            "strict RedisAuth must reject an unknown credential field with gate={enabled}"
        );
    }
}

#[test]
fn link_sys_resolved_secret_is_transport_only_and_not_recursed() {
    let doc = json!({
        "type": "webhook",
        "config": {
            "target": {"kind": "Service", "name": "resolver", "port": 8080},
            "resolvedSecrets": {"default/token:key": "private-value"}
        }
    });

    {
        let _guard = GateGuard::set(&GATE_LOCK, false);
        let spec: LinkSysSpec = serde_json::from_value(doc.clone()).expect("transport input parses with gate off");
        let SystemConfig::Webhook(webhook) = &spec.config else {
            panic!("expected webhook config");
        };
        assert_eq!(
            webhook
                .resolved_secrets
                .as_ref()
                .and_then(|values| values.0.get("default/token:key"))
                .map(String::as_str),
            Some("private-value")
        );
    }

    let _guard = GateGuard::set(&GATE_LOCK, true);
    let spec: LinkSysSpec = serde_json::from_value(doc).expect("gated boundary input still parses");
    assert_eq!(spec.unknown_fields, ["config.resolvedSecrets"]);
}
