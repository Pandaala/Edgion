//! Unit tests for the schema_check walker.
//!
//! Tests 1-7 and 9 use small hand-built local schemas (permitted by the
//! subtask spec) so each rule is pinned in isolation. Test 8 and the
//! production-schema regression test use the real
//! `schema_for!(EdgionPluginsSpec)` against realistic documents, proving the
//! walker works against the schema this feature actually ships against, not
//! only toy schemas.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::unknown_key_paths;

// ============================================================
// Test 1 — plain nested object: typo reported with dotted path
// ============================================================

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Inner {
    pub name: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Outer {
    pub inner: Inner,
}

#[test]
fn plain_nested_object_typo_reported_with_dotted_path() {
    let schema = schemars::schema_for!(Outer);
    let value = json!({"inner": {"name": "x", "extraField": 1}});

    assert_eq!(unknown_key_paths(&value, &schema), vec!["inner.extraField".to_string()]);
}

// ============================================================
// Test 2 — array element: path carries [i]
// ============================================================

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Item {
    pub name: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct Holder {
    pub items: Vec<Item>,
}

#[test]
fn array_element_path_carries_index() {
    let schema = schemars::schema_for!(Holder);
    let value = json!({"items": [{"name": "a"}, {"name": "b", "bogus": 1}]});

    assert_eq!(unknown_key_paths(&value, &schema), vec!["items[1].bogus".to_string()]);
}

// ============================================================
// Test 3 / 6 — adjacently-tagged enum (`tag = "type", content = "config"`)
// ============================================================

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct AConfig {
    pub value: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct BConfig {
    pub count: i32,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "config")]
enum Tagged {
    A(AConfig),
    B(BConfig),
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct TaggedEntry {
    #[serde(flatten)]
    pub plugin: Tagged,
}

#[test]
fn adjacently_tagged_enum_typo_inside_config_is_reported() {
    // This is the case `serde_ignored` cannot see: the tag-dispatching
    // deserializer buffers `config` before the tag is resolved, so a typo
    // inside it never reaches an "ignored key" callback. Walking the schema
    // instead catches it because the discriminator ("type": "A") selects
    // the AConfig branch and its own `properties` set excludes "typo".
    let schema = schemars::schema_for!(TaggedEntry);
    let value = json!({"type": "A", "config": {"value": "x", "typo": 1}});

    assert_eq!(unknown_key_paths(&value, &schema), vec!["config.typo".to_string()]);
}

#[test]
fn unknown_type_discriminator_reports_only_keys_unknown_in_every_branch_no_panic() {
    // "Zzz" matches neither branch's const discriminator: the walker must
    // not guess, so it does not recurse into `config` at all (a typo that
    // exists in only one candidate's shape, e.g. AConfig-shaped "value",
    // would go unreported here — that is the intended fail-silent
    // behavior). It DOES report "extra", which is unknown to both branches.
    let schema = schemars::schema_for!(TaggedEntry);
    let value = json!({"type": "Zzz", "config": {"value": "x"}, "extra": 1});

    assert_eq!(unknown_key_paths(&value, &schema), vec!["extra".to_string()]);
}

// ============================================================
// Test 4 — flattened sub-struct: a typo in the flattened region is reported
// ============================================================

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ConnPart {
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ForwardAuthShapedConfig {
    #[serde(flatten)]
    pub conn: ConnPart,
    pub decision: String,
}

#[test]
fn flattened_sub_struct_typo_is_reported() {
    // Mirrors the real ForwardAuthConfig shape: `conn` is
    // `#[serde(flatten)]`ed, so `target`/`timeoutMs` land as direct siblings
    // of `decision` in both the wire format and the schemars output. A typo
    // at that same level must be caught exactly like an unflattened field.
    let schema = schemars::schema_for!(ForwardAuthShapedConfig);
    let value = json!({"target": "svc", "decision": "d", "successStatusCodes": [200]});

    assert_eq!(
        unknown_key_paths(&value, &schema),
        vec!["successStatusCodes".to_string()]
    );
}

// ============================================================
// Test 5 — free-form map config: silent
// ============================================================

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct FreeFormHolder {
    pub map: HashMap<String, serde_json::Value>,
}

#[test]
fn free_form_map_config_is_silent() {
    // Load-bearing: this is the only thing standing between the
    // "properties present, additionalProperties absent => closed" reading
    // and a flood of false positives on every genuinely free-form plugin
    // config (Wasm plugin_config, Dsl bytecode metadata, CtxSet transforms).
    let schema = schemars::schema_for!(FreeFormHolder);
    let value = json!({"map": {"anything": 1, "whatever": {"nested": true}, "another.odd-key": null}});

    assert!(unknown_key_paths(&value, &schema).is_empty());
}

// ============================================================
// Test 7 — null for an Option<T> object field: silent
// ============================================================

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct OptionalInnerHolder {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inner: Option<Inner>,
}

#[test]
fn null_for_option_object_field_is_silent() {
    let schema = schemars::schema_for!(OptionalInnerHolder);
    let value = json!({"inner": null});

    assert!(unknown_key_paths(&value, &schema).is_empty());
}

// ============================================================
// Test 9 — caps: a pathologically deep/wide document terminates cleanly
// ============================================================

#[derive(Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
struct Recursive {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child: Option<Box<Recursive>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wide: Vec<Recursive>,
}

use crate::resources::edgion_config_data::{edgion_config_data_spec_schema, EdgionConfigDataSpec};
use crate::resources::edgion_plugins::plugin_configs::{MAX_JSON_INSTANCE_DEPTH, MAX_JSON_INSTANCE_NODES};
use crate::resources::link_sys::link_sys_spec_schema;

#[test]
fn pathologically_deep_document_terminates_and_stays_bounded() {
    // Build a chain deeper than MAX_JSON_INSTANCE_DEPTH, each level carrying
    // one legitimate key (`child`) and one typo (`extraField`), so the
    // total-typo count if fully walked would exceed the depth cap.
    let total_levels = MAX_JSON_INSTANCE_DEPTH + 50;
    let mut value = json!({"extraField": 0});
    for i in 1..total_levels {
        value = json!({"child": value, "extraField": i});
    }

    let schema = schemars::schema_for!(Recursive);
    let findings = unknown_key_paths(&value, &schema);

    // Terminates (this line is reached at all) and never reports every
    // level's typo — only however many the depth cap let it reach.
    assert!(
        findings.len() < total_levels,
        "expected the walk to stop before the deepest level, got {} findings",
        findings.len()
    );
    assert!(
        !findings.is_empty(),
        "the shallow, in-bounds typos must still be reported"
    );
}

#[test]
fn wide_flat_object_of_unknown_sibling_keys_stays_bounded() {
    // Regression: `node_count` used to advance only when `walk` recursed, and
    // an unknown key never recurses. A flat object whose keys are all
    // unrecognized therefore bypassed the cap entirely and produced one finding
    // per key — for the exact shape this feature targets (a config block full
    // of typos). The sibling-width case is deliberately separate from the
    // array-width case below: array elements DO recurse, so that test passed
    // while this shape was unbounded.
    #[derive(schemars::JsonSchema)]
    #[allow(dead_code)]
    struct OneProperty {
        known: Option<String>,
    }

    let mut obj = serde_json::Map::new();
    obj.insert("known".to_string(), json!("value"));
    for i in 0..(crate::resources::edgion_plugins::plugin_configs::MAX_JSON_INSTANCE_NODES * 2) {
        // usize loop var over a u64 bound: the cast is safe on every supported target.
        obj.insert(format!("bogus{i}"), json!(true));
    }
    let schema = schemars::schema_for!(OneProperty);
    let findings = unknown_key_paths(&serde_json::Value::Object(obj), &schema);

    assert!(
        !findings.is_empty(),
        "the unknown siblings must still be reported, just bounded"
    );
    assert!(
        findings.len() as u64 <= crate::resources::edgion_plugins::plugin_configs::MAX_JSON_INSTANCE_NODES,
        "findings must stay within the node bound, got {}",
        findings.len()
    );
}

#[test]
fn pathologically_wide_document_terminates_and_stays_bounded() {
    // A single array with far more elements than MAX_JSON_INSTANCE_NODES,
    // each carrying a typo. A full walk would produce one finding per
    // element; the node cap must stop it well short of that.
    let total_elements = (MAX_JSON_INSTANCE_NODES as usize) + 5_000;
    let elements: Vec<serde_json::Value> = (0..total_elements).map(|i| json!({"extraField": i})).collect();
    let value = json!({"wide": elements});

    let schema = schemars::schema_for!(Recursive);
    let findings = unknown_key_paths(&value, &schema);

    assert!(
        findings.len() < total_elements,
        "expected the walk to stop before the last element, got {} findings",
        findings.len()
    );
}

// ============================================================
// Test 8 — real schema_for!(EdgionPluginsSpec): a legitimate, fully
// populated config across ~10 real plugin types produces zero findings.
// ============================================================

use crate::resources::edgion_plugins::plugin_configs::{
    DebugAccessLogToHeaderConfig, DirectEndpointConfig, DslConfig, FaultInjectionConfig, GeoIpLocationConfig,
    IpRestrictionConfig, ProxyRewriteConfig, RateLimitLocalConfig, RequestBodyBufferConfig, RequestRestrictionConfig,
};
use crate::resources::edgion_plugins::{EdgionPlugin, EdgionPluginsSpec, RequestFilterEntry};

#[test]
fn real_schema_legitimate_config_across_ten_plugin_types_has_zero_findings() {
    let entries = vec![
        RequestFilterEntry::new(EdgionPlugin::DirectEndpoint(DirectEndpointConfig::default())),
        RequestFilterEntry::new(EdgionPlugin::Dsl(DslConfig::default())),
        RequestFilterEntry::new(EdgionPlugin::ProxyRewrite(ProxyRewriteConfig::default())),
        RequestFilterEntry::new(EdgionPlugin::RateLimitLocal(RateLimitLocalConfig::default())),
        RequestFilterEntry::new(EdgionPlugin::RequestRestriction(RequestRestrictionConfig::default())),
        RequestFilterEntry::new(EdgionPlugin::GeoIpLocation(GeoIpLocationConfig::default())),
        RequestFilterEntry::new(EdgionPlugin::IpRestriction(IpRestrictionConfig::default())),
        RequestFilterEntry::new(EdgionPlugin::FaultInjection(FaultInjectionConfig::default())),
        RequestFilterEntry::new(EdgionPlugin::DebugAccessLogToHeader(
            DebugAccessLogToHeaderConfig::default(),
        )),
        RequestFilterEntry::new(EdgionPlugin::RequestBodyBuffer(RequestBodyBufferConfig::default())),
    ];

    let spec = EdgionPluginsSpec {
        current_status: None,
        request_plugins: Some(entries),
        ..Default::default()
    };

    let schema = schemars::schema_for!(EdgionPluginsSpec);
    let value = serde_json::to_value(&spec).expect("EdgionPluginsSpec always serializes");

    assert_eq!(unknown_key_paths(&value, &schema), Vec::<String>::new());
}

#[test]
fn real_schema_ip_restriction_denys_typo_is_reported_with_exact_path() {
    // Regression pin for the production schema: `denys` (typo for `deny`)
    // inside an IpRestriction plugin's config, at the position a real
    // operator document would carry it.
    let schema = schemars::schema_for!(EdgionPluginsSpec);
    let value = json!({
        "requestPlugins": [
            {
                "type": "IpRestriction",
                "config": {
                    "denys": ["1.2.3.4/32"],
                    "defaultAction": "allow",
                }
            }
        ]
    });

    assert_eq!(
        unknown_key_paths(&value, &schema),
        vec!["requestPlugins[0].config.denys".to_string()]
    );
}

// ============================================================
// Regression — internally-tagged enum (`#[serde(tag = "type")]`, no
// `content`): the branch node itself carries `$ref` PLUS sibling keywords
// (the discriminator `properties.type.const`). A naive `$ref` resolver that
// REPLACES the node with its target discards those sibling keywords,
// which breaks discriminator matching (no branch ever matches, so it falls
// into the ambiguous-union path) and drops the discriminator key itself out
// of the allowed set (it lives only in the discarded siblings) — producing a
// false-positive report of the discriminator key on every use. This mirrors
// `edgion_plugins::conditions::Condition`, which is exactly this shape.
// ============================================================

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct KeyExistVariant {
    pub key: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct KeyMatchVariant {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
enum InternallyTaggedCondition {
    KeyExist(KeyExistVariant),
    KeyMatch(KeyMatchVariant),
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ConditionHolder {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<Vec<InternallyTaggedCondition>>,
}

#[test]
fn internally_tagged_enum_reports_typo_inside_variant_and_not_the_discriminator() {
    let schema = schemars::schema_for!(InternallyTaggedCondition);
    let value = json!({"type": "keyMatch", "key": "x", "typo": 1});

    assert_eq!(unknown_key_paths(&value, &schema), vec!["typo".to_string()]);
}

#[test]
fn internally_tagged_enum_nested_reports_exact_indexed_path() {
    // Synthetic array holder proving indexed traversal of internally-tagged
    // enum elements. Product ConditionSet shapes are covered below.
    let schema = schemars::schema_for!(ConditionHolder);
    let value = json!({
        "run": [
            {"type": "keyMatch", "key": "x", "value": "1"},
            {"type": "keyExist", "key": "y", "typo": true},
        ]
    });

    assert_eq!(unknown_key_paths(&value, &schema), vec!["run[1].typo".to_string()]);
}

#[test]
fn real_schema_internally_tagged_conditions_run_has_zero_findings() {
    // The exact corpus shape that regressed: a well-formed `conditions.run`
    // block built from the real `Condition` enum, walked against the real
    // `schema_for!(EdgionPluginsSpec)`. Before the `$ref`-with-siblings fix,
    // this produced a false positive on
    // `requestPlugins[0].conditions.run.allOf[0].type`.
    let schema = schemars::schema_for!(EdgionPluginsSpec);
    let value = json!({
        "requestPlugins": [
            {
                "type": "IpRestriction",
                "config": {"defaultAction": "allow"},
                "conditions": {
                    "run": {
                        "allOf": [
                            {"type": "keyMatch", "key": {"type": "header", "name": "X-Test"}, "values": ["1"]}
                        ]
                    }
                }
            }
        ]
    });

    assert_eq!(unknown_key_paths(&value, &schema), Vec::<String>::new());
}

#[test]
fn unknown_key_paths_reports_typo_inside_flattened_match_core_on_real_condition_schema() {
    // First production `#[serde(flatten)]` on Condition: a typo inside the
    // flattened MatchCore region (`valuez` / `regexx` on a `keyMatch`) must
    // be reported against the real EdgionPlugins spec schema.
    let schema = crate::resources::edgion_plugins::edgion_plugins_spec_schema();
    let value = json!({
        "requestPlugins": [
            {
                "type": "IpRestriction",
                "config": {"defaultAction": "allow"},
                "conditions": {
                    "run": {
                        "anyOf": [
                            {
                                "type": "keyMatch",
                                "key": {"type": "header", "name": "X-Test"},
                                "values": ["ok"],
                                "valuez": "typo",
                                "regexx": ["^x"]
                            }
                        ]
                    }
                }
            }
        ]
    });
    let findings = unknown_key_paths(&value, schema);
    assert!(
        findings.iter().any(|p| p.contains("valuez")),
        "flattened MatchCore typo valuez must be reported, got {findings:?}"
    );
    assert!(
        findings.iter().any(|p| p.contains("regexx")),
        "flattened MatchCore typo regexx must be reported, got {findings:?}"
    );
}

// ============================================================
// EdgionConfigData production-schema boundaries
// ============================================================

#[test]
fn config_data_typed_variants_report_exact_nested_paths() {
    let cases = [
        (
            json!({"data":{"type":"KeyList","config":{"matchMode":"exact","items":[{"name":"g","items":[{"key":"x","behaviour":"drop"}]}]}}}),
            "data.config.items[0].items[0].behaviour",
        ),
        (
            json!({"data":{"type":"IpList","config":{"items":[{"name":"g","cidrs":["10.0.0.0/8"],"cidrz":["10.0.0.0/8"]}]}}}),
            "data.config.items[0].cidrz",
        ),
        (
            json!({"data":{"type":"RequestAccessUrlAllowList","config":{"items":[{"name":"health","hosts":["api.example.com"],"paths":[{"type":"Exact","value":"/health","valuez":"/ignored"}],"conditions":{"allOf":[]}}]}}}),
            "data.config.items[0].paths[0].valuez",
        ),
        (
            json!({"data":{"type":"Selector","config":{"active":"lockdown","descriptin":"x"}}}),
            "data.config.descriptin",
        ),
        (
            json!({"data":{"type":"RegionRouteOverride","config":{"enable":true,"regionz":[]}}}),
            "data.config.regionz",
        ),
    ];

    for (value, expected) in cases {
        serde_json::from_value::<EdgionConfigDataSpec>(value.clone())
            .expect("a document containing only an advisory typo must remain deserializable");
        assert_eq!(
            unknown_key_paths(&value, edgion_config_data_spec_schema()),
            vec![expected.to_string()]
        );
    }
}

#[test]
fn config_data_clean_typed_variants_have_no_findings() {
    let cases = [
        json!({"data":{"type":"KeyList","config":{"matchMode":"exact","items":[{"name":"g","items":[{"key":"x"}]}]}}}),
        json!({"data":{"type":"IpList","config":{"items":[{"name":"g","cidrs":["10.0.0.0/8"]}]}}}),
        json!({"data":{"type":"RequestAccessUrlAllowList","config":{"items":[{"name":"health","hosts":["api.example.com"],"paths":[{"type":"Exact","value":"/health"}],"conditions":{"allOf":[]}}]}}}),
        json!({"data":{"type":"Selector","config":{"active":"lockdown","description":"x"}}}),
        json!({"data":{"type":"RegionRouteOverride","config":{"enable":true,"regions":null}}}),
    ];

    for value in cases {
        serde_json::from_value::<EdgionConfigDataSpec>(value.clone())
            .expect("a clean typed config must deserialize through the production type");
        assert!(
            unknown_key_paths(&value, edgion_config_data_spec_schema()).is_empty(),
            "clean typed config must not produce findings: {value}"
        );
    }
}

#[test]
fn config_data_misc_config_is_open_but_data_siblings_are_not() {
    let value = json!({
        "data": {
            "type": "Misc",
            "config": {"arbitrary": {"nested": [{"anything": true}]}},
            "typo": true
        }
    });
    assert_eq!(
        unknown_key_paths(&value, edgion_config_data_spec_schema()),
        vec!["data.typo".to_string()]
    );
}

#[test]
fn config_data_envelope_typo_is_reported() {
    let value = json!({"enabel": true, "data":{"type":"Misc","config":{}}});
    assert_eq!(
        unknown_key_paths(&value, edgion_config_data_spec_schema()),
        vec!["enabel".to_string()]
    );
}

#[test]
fn config_data_missing_or_unknown_discriminator_is_fail_silent_but_serde_rejects() {
    for value in [
        json!({"data":{"config":{"typo":true}}}),
        json!({"data":{"type":"FutureType","config":{"typo":true}}}),
    ] {
        assert!(unknown_key_paths(&value, edgion_config_data_spec_schema()).is_empty());
        assert!(serde_json::from_value::<EdgionConfigDataSpec>(value).is_err());
    }
}

#[test]
fn config_data_generated_schema_keeps_only_misc_open() {
    let schema = edgion_config_data_spec_schema().as_value();
    let branches = schema["$defs"]["ConfigEntry"]["oneOf"]
        .as_array()
        .expect("ConfigEntry must remain an adjacent-tagged oneOf");
    assert_eq!(branches.len(), 7);
    let mut saw_misc = false;
    let mut typed_branches = 0usize;

    for branch in branches {
        let variant = branch["properties"]["type"]["const"]
            .as_str()
            .expect("each ConfigEntry branch must pin its discriminator");
        let config = &branch["properties"]["config"];
        if variant == "Misc" {
            saw_misc = true;
            assert_eq!(config, &json!(true), "Misc.config must remain unconstrained");
        } else {
            typed_branches += 1;
            assert!(
                config.get("$ref").and_then(serde_json::Value::as_str).is_some(),
                "typed {variant}.config must remain schema-backed, got {config}"
            );
        }
    }
    assert!(saw_misc, "ConfigEntry must retain its Misc escape hatch");
    assert!(
        typed_branches > 0,
        "ConfigEntry must retain schema-backed typed variants"
    );
}

#[test]
fn link_sys_schema_dispatches_all_provider_variants() {
    for (provider, config) in [
        ("redis", json!({"endpoints": []})),
        ("etcd", json!({"endpoints": []})),
        ("elasticsearch", json!({"endpoints": []})),
        ("kafka", json!({"brokers": []})),
        ("webhook", json!({})),
        ("httpdns", json!({})),
    ] {
        let mut value = json!({"type": provider, "config": config});
        assert!(
            unknown_key_paths(&value, link_sys_spec_schema()).is_empty(),
            "clean provider {provider} must not produce findings: {value}"
        );
        value["config"]["typo"] = json!(true);
        assert_eq!(
            unknown_key_paths(&value, link_sys_spec_schema()),
            vec!["config.typo".to_string()],
            "provider {provider} must select its typed config schema"
        );
    }
}

#[test]
fn link_sys_reports_deep_typo_and_keeps_open_boundaries_quiet() {
    let redis = json!({
        "type": "redis",
        "config": {
            "endpoints": [],
            "timeout": {"connect": 10, "conect": 20},
            "observability": {"metrics": {"labels": {"team": "edge", "arbitrary": "value"}}}
        }
    });
    assert_eq!(
        unknown_key_paths(&redis, link_sys_spec_schema()),
        vec!["config.timeout.conect".to_string()]
    );

    let webhook = json!({
        "type": "webhook",
        "config": {
            "auth": {"legacy": {"anything": true}},
            "defaultHeaders": {"X-Legacy": {"anything": true}},
            "requestMethod": {"anything": true},
            "unknownSibling": true
        }
    });
    assert_eq!(
        unknown_key_paths(&webhook, link_sys_spec_schema()),
        vec!["config.unknownSibling".to_string()]
    );
}
