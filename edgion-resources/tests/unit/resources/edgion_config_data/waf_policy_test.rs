use super::*;
use crate::resources::edgion_config_data::{ConfigEntry, EdgionConfigData, EdgionConfigDataSpec};

fn policy() -> WafPolicyData {
    WafPolicyData {
        default_profile: "low".into(),
        profiles: BTreeMap::from([(
            "low".into(),
            WafProfile {
                rule_selection: Default::default(),
                bundle_refs: vec![WafBundleRef {
                    target: EdgionConfigDataRef {
                        name: "base".into(),
                        ..Default::default()
                    },
                    expected_digest: Some("a".repeat(64)),
                    optional: false,
                }],
            },
        )]),
    }
}

#[test]
fn profiles_require_a_named_default_and_required_base() {
    let value = policy();
    assert!(value.validate_shape().is_ok());
    let mut missing_default = value.clone();
    missing_default.default_profile = "missing".into();
    assert!(missing_default.validate_shape().is_err());
    let mut optional_only = value.clone();
    optional_only.profiles.get_mut("low").unwrap().bundle_refs[0].optional = true;
    assert!(optional_only.validate_shape().is_err());
    let mut unpinned = value;
    unpinned.profiles.get_mut("low").unwrap().bundle_refs[0].expected_digest = None;
    assert!(unpinned.validate_shape().is_ok());
    let encoded = serde_json::to_value(&unpinned).unwrap();
    assert!(encoded["profiles"]["low"]["bundleRefs"][0]
        .get("expectedDigest")
        .is_none());
    unpinned.profiles.get_mut("low").unwrap().bundle_refs[0].expected_digest = Some("invalid".into());
    assert!(unpinned.validate_shape().is_err());
    unpinned.profiles.get_mut("low").unwrap().bundle_refs[0].expected_digest = Some(String::new());
    assert!(unpinned.validate_shape().is_err());
}

#[test]
fn policy_transport_preserves_profiles_and_rejects_unknown_config() {
    let value = policy();
    let encoded = serde_json::to_value(&value).unwrap();
    let decoded: WafPolicyData = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(decoded.default_profile, "low");
    assert!(decoded.profiles.contains_key("low"));
    let mut invalid = encoded;
    invalid["activeProfileRef"] = serde_json::json!({"name": "selector"});
    assert!(serde_json::from_value::<WafPolicyData>(invalid).is_err());
}

#[test]
fn policy_resource_requires_enabled_unambiguous_envelope() {
    let mut resource = EdgionConfigData::new(
        "policy",
        EdgionConfigDataSpec {
            current_status: None,
            enable: true,
            active: None,
            visibility: Default::default(),
            data: ConfigEntry::WafPolicy(policy()),
            unknown_fields: vec![],
        },
    );
    assert_eq!(resource.data_type(), "WafPolicy");
    assert!(resource.validate_pre_parse().is_empty());
    resource.spec.enable = false;
    assert!(!resource.validate_pre_parse().is_empty());
    resource.spec.enable = true;
    resource.spec.active = Some("low".into());
    assert!(!resource.validate_pre_parse().is_empty());
    resource.spec.active = None;
    resource.spec.unknown_fields.push("data.config.defaultProfiles".into());
    assert!(!resource.validate_pre_parse().is_empty());
}

#[test]
fn selection_transport_defaults_and_exact_prepared_binding() {
    let mut value = policy();
    let encoded = serde_json::to_value(&value).unwrap();
    assert!(encoded["profiles"]["low"].get("ruleSelection").is_none());
    for invalid in [serde_json::Value::Null, serde_json::json!({"excludeTags": ["php"]})] {
        let mut invalid_wire = encoded.clone();
        invalid_wire["profiles"]["low"]["ruleSelection"] = invalid;
        assert!(serde_json::from_value::<WafPolicyData>(invalid_wire).is_err());
    }
    value.profiles.get_mut("low").unwrap().rule_selection = yaml_serde::from_str("include: {tag: php}").unwrap();
    let prepared = value.prepare_selections().unwrap();
    assert!(prepared.live_bytes() > 0);
    assert!(prepared.selection("low", &value.profiles["low"]).unwrap().is_some());
    let decoded: WafPolicyData = serde_json::from_value(serde_json::to_value(&value).unwrap()).unwrap();
    assert_eq!(
        decoded.profiles["low"].rule_selection,
        value.profiles["low"].rule_selection
    );
    value.profiles.get_mut("low").unwrap().rule_selection = yaml_serde::from_str("include: {tag: java}").unwrap();
    assert!(prepared.selection("low", &value.profiles["low"]).is_err());
    let schema = serde_json::to_value(schemars::schema_for!(WafProfile)).unwrap();
    assert!(schema["properties"].get("ruleSelection").is_some());
}

#[test]
fn mixed_policy_empty_profile_is_one_pass_and_stale_sibling_binding_is_rejected() {
    let mut value = policy();
    let mut sibling = value.profiles["low"].clone();
    sibling.rule_selection = yaml_serde::from_str("include: {tag: php}").unwrap();
    value.profiles.insert("other".into(), sibling);
    let owner = value.prepare_selections().unwrap();
    assert!(owner.validate_binding(&value).is_ok());
    assert!(owner.selection("low", &value.profiles["low"]).unwrap().is_none());
    value.profiles.get_mut("other").unwrap().rule_selection =
        yaml_serde::from_str("include: {anyOf: [{tag: php}, {tag: java}]}").unwrap();
    assert!(owner.validate_binding(&value).is_err());
}
