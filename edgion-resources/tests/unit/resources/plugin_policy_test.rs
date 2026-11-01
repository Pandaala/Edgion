use super::*;
use crate::resources::edgion_plugins::RequestFilterEntry;

fn compile_yaml(yaml: &str) -> Result<CompiledPluginPolicy, String> {
    yaml_serde::from_str::<PluginPolicyConfig>(yaml).unwrap().compile()
}

#[test]
fn gateway_policy_defaults_and_explicit_empty_allow_differ() {
    let absent: crate::EdgionGatewayConfigSpec = yaml_serde::from_str("{}").unwrap();
    assert!(absent.plugin_policy.is_none());
    assert!(absent.validate_plugin_policy().is_empty());
    assert_eq!(compile_yaml("{}").unwrap(), CompiledPluginPolicy::default());
    let empty_allow = compile_yaml("allow: []\ndefaultAction: allow").unwrap();
    for (family, name) in [
        (PluginFamily::Http, "Wasm"),
        (PluginFamily::Stream, "ConnectionRateLimit"),
        (PluginFamily::TlsRoute, "IpRestriction"),
    ] {
        assert_eq!(CompiledPluginPolicy::default().action_for(family, name), None);
        assert_eq!(
            empty_allow.action_for(family, name),
            Some(PluginPolicyAction::Block { status: 503 })
        );
    }
    assert_eq!(
        empty_allow.action_for(PluginFamily::Http, "FutureType"),
        Some(PluginPolicyAction::Block { status: 503 })
    );
    let roundtrip: PluginPolicyConfig = yaml_serde::from_str("allow: []").unwrap();
    assert_eq!(serde_json::to_value(roundtrip).unwrap()["allow"], serde_json::json!([]));
    assert!(serde_json::to_value(PluginPolicyConfig::default())
        .unwrap()
        .get("allow")
        .is_none());
}

#[test]
fn gateway_policy_deny_overrides_allow_and_allow_overrides_default_deny() {
    let policy = compile_yaml(
        "allow: [http/Wasm, http/Dsl]\ndefaultAction: deny\ndeny:\n  - name: http/Wasm\n    blockStatus: 451",
    )
    .unwrap();
    assert_eq!(
        policy.action_for(PluginFamily::Http, "Wasm"),
        Some(PluginPolicyAction::Block { status: 451 })
    );
    assert_eq!(policy.action_for(PluginFamily::Http, "Dsl"), None);
    assert_eq!(
        policy.action_for(PluginFamily::Http, "Mock"),
        Some(PluginPolicyAction::Block { status: 503 })
    );
    let default_deny = compile_yaml("defaultAction: deny\ndeniedAction: bypass").unwrap();
    assert_eq!(
        default_deny.action_for(PluginFamily::Http, "Mock"),
        Some(PluginPolicyAction::Bypass)
    );
}

#[test]
fn gateway_policy_inherits_status_and_actions_across_families() {
    let policy = compile_yaml("blockStatus: 451\ndeny:\n  - name: http/Wasm\n  - name: stream/ConnectionRateLimit\n    action: bypass\n  - name: tls-route/IpRestriction\n    blockStatus: 403").unwrap();
    assert_eq!(
        policy.action_for(PluginFamily::Http, "Wasm"),
        Some(PluginPolicyAction::Block { status: 451 })
    );
    assert_eq!(
        policy.action_for(PluginFamily::Stream, "ConnectionRateLimit"),
        Some(PluginPolicyAction::Bypass)
    );
    assert_eq!(
        policy.action_for(PluginFamily::TlsRoute, "IpRestriction"),
        Some(PluginPolicyAction::Block { status: 403 })
    );
    assert_eq!(policy.action_for(PluginFamily::Http, "IpRestriction"), None);

    let bypass = compile_yaml(
        "deniedAction: bypass\nblockStatus: 451\ndeny:\n  - name: http/Wasm\n  - name: http/Dsl\n    action: block",
    )
    .unwrap();
    assert_eq!(
        bypass.action_for(PluginFamily::Http, "Wasm"),
        Some(PluginPolicyAction::Bypass)
    );
    assert_eq!(
        bypass.action_for(PluginFamily::Http, "Dsl"),
        Some(PluginPolicyAction::Block { status: 451 })
    );
}

#[test]
fn gateway_policy_rejects_invalid_status_and_bypass_status_override() {
    for status in [400, 599] {
        assert!(compile_yaml(&format!(
            "blockStatus: {status}\ndeny:\n  - name: http/Wasm\n    blockStatus: {status}"
        ))
        .is_ok());
    }
    for status in [0, 399, 600, u16::MAX] {
        assert!(compile_yaml(&format!("blockStatus: {status}")).is_err());
        assert!(compile_yaml(&format!("deny:\n  - name: http/Wasm\n    blockStatus: {status}")).is_err());
    }
    for yaml in [
        "deny:\n  - name: http/Wasm\n    action: bypass\n    blockStatus: 451",
        "deniedAction: bypass\ndeny:\n  - name: http/Wasm\n    blockStatus: 451",
    ] {
        assert!(compile_yaml(yaml).unwrap_err().contains("must not be set for bypass"));
    }
    assert!(
        compile_yaml("deniedAction: bypass\ndeny:\n  - name: http/Wasm\n    action: block\n    blockStatus: 451")
            .is_ok()
    );
}

#[test]
fn gateway_policy_rejects_duplicate_and_noncanonical_names() {
    for name in [
        "http/wasm",
        "grpc/Wasm",
        "http/Unknown",
        "http/*",
        "http/Waf",
        "http/Wasm/extra",
        "Wasm",
    ] {
        assert!(compile_yaml(&format!("allow: [{name}]")).is_err(), "{name}");
        assert!(compile_yaml(&format!("deny:\n  - name: {name}")).is_err(), "{name}");
    }
    for yaml in [
        "allow: [http/Wasm, http/Wasm]",
        "deny:\n  - name: http/Wasm\n  - name: http/Wasm\n    action: bypass",
    ] {
        assert!(compile_yaml(yaml).unwrap_err().contains("duplicate"));
    }
}

#[test]
fn gateway_policy_serde_rejects_unknown_fields_types_and_legacy_spelling() {
    for yaml in [
        "mode: denylist",
        "default_action: deny",
        "defaultAction: Allow",
        "deniedAction: allow",
        "blockStatus: bad",
        "blockStatus: 65536",
        "allow: http/Wasm",
        "deny:\n  - name: http/Wasm\n    block_status: 451",
        "deny:\n  - name: http/Wasm\n    enable: false",
        "deny:\n  - action: block",
    ] {
        assert!(yaml_serde::from_str::<PluginPolicyConfig>(yaml).is_err(), "{yaml}");
    }
    let cfg: PluginPolicyConfig = yaml_serde::from_str("deny:\n  - name: http/Wasm").unwrap();
    let wire = serde_json::to_value(&cfg).unwrap();
    assert!(wire["deny"][0].get("action").is_none());
    assert!(wire["deny"][0].get("blockStatus").is_none());
    assert_eq!(serde_json::from_value::<PluginPolicyConfig>(wire).unwrap(), cfg);
}

#[test]
fn execution_action_serde_round_trip() {
    let block = PluginPolicyAction::Block { status: 403 };
    let json = serde_json::to_string(&block).unwrap();
    assert_eq!(json, r#"{"action":"block","status":403}"#);
    let back: PluginPolicyAction = serde_json::from_str(&json).unwrap();
    assert_eq!(back, block);

    let bypass = PluginPolicyAction::Bypass;
    let json = serde_json::to_string(&bypass).unwrap();
    assert_eq!(json, r#"{"action":"bypass"}"#);
}

#[test]
fn plugin_entries_do_not_carry_execution_policy() {
    let entry: RequestFilterEntry = yaml_serde::from_str("type: Mock\nconfig: {}").unwrap();
    let wire = serde_json::to_value(&entry).unwrap();
    let schema = serde_json::to_value(schemars::schema_for!(RequestFilterEntry)).unwrap();
    assert!(wire.get("policyAction").is_none());
    assert!(schema["properties"].get("policyAction").is_none());
}

#[test]
fn parse_qualified_name_accepts_all_families() {
    assert_eq!(
        parse_qualified_name("http/Wasm").unwrap(),
        (PluginFamily::Http, "Wasm".to_string())
    );
    assert_eq!(
        parse_qualified_name("stream/ConnectionRateLimit").unwrap(),
        (PluginFamily::Stream, "ConnectionRateLimit".to_string())
    );
    assert_eq!(
        parse_qualified_name("tls-route/IpRestriction").unwrap(),
        (PluginFamily::TlsRoute, "IpRestriction".to_string())
    );
}

#[test]
fn parse_qualified_name_rejects_bad_input() {
    for bad in ["Wasm", "http/", "/Wasm", "grpc/Wasm", "http/Wasm/extra", ""] {
        assert!(parse_qualified_name(bad).is_err(), "should reject {bad:?}");
    }
}

#[test]
fn canonical_type_names_extracts_all_three_families() {
    let http = canonical_type_names(PluginFamily::Http);
    assert!(http.contains("Wasm") && http.contains("RequestHeaderModifier"));
    assert!(http.len() >= 30, "http family too small: {}", http.len());

    let stream = canonical_type_names(PluginFamily::Stream);
    assert!(stream.contains("ConnectionRateLimit"));
    assert!(!stream.is_empty());

    let tls_route = canonical_type_names(PluginFamily::TlsRoute);
    assert!(tls_route.contains("IpRestriction"));
    assert!(!tls_route.is_empty());
}
