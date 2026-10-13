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
//! a plugin's `resolved_*` credential map) travels only
//! over conf_sync to the gateway, where the gate is always `false`. If a
//! future change ever feeds a resolved document into a gated seam, its
//! `#[schemars(skip)]` fields would surface as false-positive findings —
//! that is the tripwire to watch for. See `realistic_reingest_round_trip_has_zero_findings`
//! and `operator_written_unknown_entry_field_is_reported` below for why
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
            "policyRef": {"name": "application-waf"},
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
// `resolved_*` credential fields, e.g.
// `KeyAuthConfig::resolved_keys`) also carries `skip_serializing_if`, so a
// freshly-parsed, NOT-YET-RESOLVED object serializes without any of them.
// The only re-ingest that actually happens at a gated seam is: Admin API
// parses an operator document, persists `serde_json::to_string(&obj)`
// verbatim (FS/etcd), and the loader reads it back later. This models
// exactly that path — deserialize an operator document (with an auth
// plugin that HAS a `resolved_*` field, left at its natural `None`), turn
// it into a `Value`, and feed the value back through the gated path.
//
// Deliberately does NOT hand-populate any `resolved_*`
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
                "config": {"defaultAction": "allow", "denyRefs": [{"name": "blocklist"}]}
            }
        ]
    });

    // First parse: an operator-authored document, no `resolved_*`
    // fields present — exactly what an Admin API write
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
// Test 4b — an unknown entry-level control is reported rather than silently
// becoming an execution switch. Operator ingress does not accept runtime policy.
// ============================================================

#[test]
fn operator_written_unknown_entry_field_is_reported() {
    let _guard = GateGuard::set(&GATE_LOCK, true);

    let doc = json!({
        "requestPlugins": [
            {
                "type": "IpRestriction",
                "config": {"defaultAction": "allow"},
                "executionOverride": {"action": "bypass"}
            }
        ]
    });

    let spec: EdgionPluginsSpec = serde_json::from_value(doc).expect("well-formed aside from the operator field");
    assert_eq!(
        spec.unknown_fields,
        vec!["requestPlugins[0].executionOverride".to_string()],
        "an unknown entry control must be reported because it has no effect"
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
fn proxy_protocol_config_data_gate_on_captures_fatal_config_typo() {
    let _guard = GateGuard::set(&GATE_LOCK, true);
    let spec: EdgionConfigDataSpec = serde_json::from_value(json!({
        "data": {"type": "ProxyProtocolTrust", "config": {"mode": "required", "typo": true}}
    }))
    .expect("security policy typo must remain deserializable for status reporting");
    assert_eq!(spec.unknown_fields, ["data.config.typo"]);
    assert!(spec
        .canonical_proxy_protocol_trust_policy()
        .expect("ProxyProtocolTrust variant")
        .is_err());
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
                    "conditions": {"allOf": [{"type": "directPeerIpMatch", "ipRefs": [{"name":"networks"}], "ipsz": []}]}
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
fn link_sys_redis_removed_fields_are_reported_as_unknown() {
    let _guard = GateGuard::set(&GATE_LOCK, true);
    let spec: LinkSysSpec = serde_json::from_value(json!({
        "type": "redis",
        "config": {
            "endpoints": ["redis://127.0.0.1:6379"],
            "retry": {"maxAttempts": 3},
            "pool": {"minIdle": 1},
            "topology": {"mode": "cluster", "cluster": {"readFromReplicas": true}},
            "observability": {"metrics": {"enabled": true}}
        }
    }))
    .expect("removed Redis fields remain advisory rather than rejecting ingest");

    assert_eq!(
        spec.unknown_fields,
        [
            "config.observability",
            "config.pool.minIdle",
            "config.retry",
            "config.topology.cluster.readFromReplicas",
        ]
    );
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

#[test]
fn migrated_geo_fixtures_decode_through_canonical_resource_wire() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../edgion-tests");
    let files = [
        "integration/conf/EdgionPlugins/GeoRestriction/00_EdgionConfigData_geo-http-cidrs.yaml",
        "integration/conf/EdgionPlugins/GeoRestriction/02_EdgionPlugins_geo_rr.yaml",
        "integration/conf/EdgionPlugins/GeoRestriction/04_EdgionPlugins_geo_gac.yaml",
        "integration/conf/EdgionPlugins/GeoRestriction/06_EdgionPlugins_geo_invalid.yaml",
        "integration/conf/EdgionPlugins/GeoRestriction/07_EdgionPlugins_geo_parseonly.yaml",
        "integration/conf/TCPRoute/GeoRestriction/00_EdgionConfigData_geo-stream-cidrs.yaml",
        "integration/conf/TCPRoute/GeoRestriction/01_EdgionStreamPlugins_tcp_geo.yaml",
        "integration/conf/TLSRoute/GeoRestriction/00_EdgionConfigData_geo-tls-cidrs.yaml",
        "integration/conf/TLSRoute/GeoRestriction/01_EdgionStreamPlugins_geo_deny.yaml",
        "k8s/conf/EdgionPlugins/GeoRestriction/00_EdgionConfigData_geo-http-cidrs.yaml",
        "k8s/conf/EdgionPlugins/GeoRestriction/resources.yaml",
        "k8s/conf/TCPRoute/GeoRestriction/00_EdgionConfigData_geo-stream-cidrs.yaml",
        "k8s/conf/TCPRoute/GeoRestriction/resources.yaml",
        "k8s/conf/TLSRoute/GeoRestriction/00_EdgionConfigData_geo-tls-cidrs.yaml",
        "k8s/conf/TLSRoute/GeoRestriction/resources.yaml",
    ];
    use serde::Deserialize;
    let mut list_count = 0;
    let mut plugin_count = 0;
    for file in files {
        let text = std::fs::read_to_string(root.join(file)).unwrap();
        for document in serde_yaml::Deserializer::from_str(&text) {
            let value = serde_yaml::Value::deserialize(document).unwrap();
            match value["kind"].as_str() {
                Some("EdgionConfigData") => {
                    let data: crate::EdgionConfigData = serde_yaml::from_value(value).unwrap();
                    let crate::resources::edgion_config_data::ConfigEntry::IpList(list) = data.spec.data else {
                        panic!("named Geo fixture must be an IpList")
                    };
                    assert_eq!(list.items.len(), 1);
                    assert_eq!(list.items[0].cidrs.len(), 1);
                    assert_eq!(
                        list.items[0].cidrs[0],
                        if file.starts_with("k8s/") {
                            "192.168.0.0/16"
                        } else {
                            "127.0.0.5/32"
                        }
                    );
                    list_count += 1;
                }
                Some("EdgionPlugins") => {
                    let _: crate::EdgionPlugins = serde_yaml::from_value(value).unwrap();
                    plugin_count += 1;
                }
                Some("EdgionStreamPlugins") => {
                    let _: crate::EdgionStreamPlugins = serde_yaml::from_value(value).unwrap();
                    plugin_count += 1;
                }
                _ => {}
            }
        }
    }
    assert_eq!(list_count, 6);
    assert!(plugin_count >= 7, "native and K8s mixed-resource mirrors were decoded");
}

#[test]
fn generated_geo_cidr_literals_use_reference_objects_and_decode_canonically() {
    fn check(text: &str, is_stream: bool, expected_list: &str) {
        let node: serde_yaml::Value = serde_yaml::from_str(text).expect("generated CIDR template YAML");
        let configs: Vec<serde_yaml::Value> = if let Some(entries) = node.as_sequence() {
            entries
                .iter()
                .filter(|entry| entry["type"].as_str() == Some("GeoIpLocation"))
                .map(|entry| entry["config"].clone())
                .collect()
        } else if node["kind"].as_str().is_some() {
            let field = if is_stream { "plugins" } else { "requestPlugins" };
            node["spec"][field]
                .as_sequence()
                .unwrap()
                .iter()
                .filter(|entry| entry["type"].as_str() == Some("GeoIpLocation"))
                .map(|entry| entry["config"].clone())
                .collect()
        } else {
            vec![node]
        };
        assert!(!configs.is_empty());
        for config in configs {
            let config: crate::resources::edgion_plugins::plugin_configs::GeoIpLocationConfig =
                serde_yaml::from_value(config).unwrap();
            assert!(config.check_fields(is_stream).is_none(), "valid authored CIDR template");
            for rule in config
                .deny_rule_groups
                .iter()
                .chain(&config.allow_rule_groups)
                .flat_map(|group| &group.rules)
            {
                if rule.key == crate::resources::edgion_plugins::plugin_configs::GeoRuleKey::Cidr {
                    assert!(rule.values.is_empty());
                    assert_eq!(rule.cidr.ip_refs.len(), 1);
                    assert_eq!(rule.cidr.ip_refs[0].name, expected_list);
                }
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../edgion-tests");
    let quoted = regex::Regex::new(r#""(?:\\.|[^"\\])*""#).unwrap();
    let mut count = 0;
    for (path, is_stream, expected_list) in [
        (
            "src/client/suites/edgion_plugins/geo_restriction/geo_restriction.rs",
            false,
            "geo-http-cidrs",
        ),
        (
            "src/client/suites/tcp_route/geo_restriction/geo_restriction.rs",
            true,
            "geo-stream-cidrs",
        ),
        (
            "src/client/suites/tls_route/geo_restriction/geo_restriction.rs",
            true,
            "geo-tls-cidrs",
        ),
    ] {
        let source = std::fs::read_to_string(root.join(path)).unwrap();
        for literal in quoted.find_iter(&source) {
            let Ok(text) = serde_json::from_str::<String>(literal.as_str()) else {
                continue;
            };
            if text.contains("key: cidr\n") && text.contains("source:") {
                check(&text.replace("{{", "{").replace("}}", "}"), is_stream, expected_list);
                count += 1;
            }
        }
    }
    assert_eq!(count, 7, "all HTTP/TCP/TLS CIDR literals");
    let shell = std::fs::read_to_string(root.join("integration/scripts/integration/test_geoip_db_states.sh")).unwrap();
    let heredocs = regex::Regex::new(r"(?s)<<EOF\n(.*?)\nEOF").unwrap();
    let mut shell_count = 0;
    for block in heredocs.captures_iter(&shell) {
        let text = &block[1];
        if !text.contains("key: cidr\n") {
            continue;
        }
        let node: serde_yaml::Value = serde_yaml::from_str(text).unwrap();
        let field = if node["kind"].as_str() == Some("EdgionStreamPlugins") {
            "plugins"
        } else {
            "requestPlugins"
        };
        for entry in node["spec"][field].as_sequence().unwrap() {
            let config: crate::resources::edgion_plugins::plugin_configs::GeoIpLocationConfig =
                serde_yaml::from_value(entry["config"].clone()).unwrap();
            assert!(config.check_fields(field == "plugins").is_none());
            let rule = &config.deny_rule_groups[0].rules[0];
            assert!(rule.values.is_empty());
            assert_eq!(rule.cidr.ip_refs[0].name, "geo-cidr-peer");
        }
        shell_count += 1;
    }
    assert_eq!(shell_count, 2);
}
