use super::*;
use edgion_resources::common::KeyGet;
use edgion_resources::resources::edgion_plugins::conditions::{
    Condition, ConditionSet, KeyExistCondition, PluginConditions,
};
use edgion_resources::resources::edgion_plugins::{
    BodyStage, HmacCredential, HmacMatchConfig, RequestRestrictionConfig, RestrictionMode,
};
use std::collections::HashMap;

#[test]
fn nested_request_restriction_body_hmac_declares_plugin_level_requirement() {
    let plugin = EdgionPlugin::RequestRestriction(RequestRestrictionConfig {
        mode: Some(RestrictionMode::Allow),
        conditions: ConditionSet::all_of(vec![Condition::HmacAuth(HmacMatchConfig {
            validate_request_body: true,
            resolved_credentials: Some(HashMap::from([(
                "alice".to_string(),
                HmacCredential {
                    secret: b"body-hmac-secret-at-least-32-bytes".to_vec(),
                    headers: HashMap::new(),
                    group: "default".to_string(),
                },
            )])),
            ..Default::default()
        })]),
        ..Default::default()
    });

    let requirement = PluginRuntime::resolve_body_requirement(&plugin, None, BodyStage::Request);
    assert!(requirement.is_some());
    assert!(PluginRuntime::use_generic_body_gate(&plugin));
}

#[test]
fn invalid_deny_restriction_bypasses_body_gate_for_generic_500() {
    let unresolved = Condition::SecretMatch(
        edgion_resources::resources::edgion_plugins::conditions::SecretMatchCondition {
            key: KeyGet::Header {
                name: "X-Secret".to_string(),
            },
            secret_key: "token".to_string(),
            secret_ref: edgion_resources::resources::common::SecretObjectReference {
                name: "missing".to_string(),
                group: None,
                kind: None,
                namespace: None,
            },
            negate: false,
            resolved_values: None,
            descriptor: Box::from(""),
        },
    );
    let body_hmac = Condition::HmacAuth(HmacMatchConfig {
        validate_request_body: true,
        resolved_credentials: Some(HashMap::from([(
            "alice".to_string(),
            HmacCredential {
                secret: b"body-hmac-secret-at-least-32-bytes".to_vec(),
                headers: HashMap::new(),
                group: "default".to_string(),
            },
        )])),
        ..Default::default()
    });
    let plugin = EdgionPlugin::RequestRestriction(RequestRestrictionConfig {
        mode: Some(RestrictionMode::Deny),
        conditions: ConditionSet::all_of(vec![unresolved, body_hmac]),
        ..Default::default()
    });

    assert!(PluginRuntime::resolve_body_requirement(&plugin, None, BodyStage::Request).is_some());
    assert!(!PluginRuntime::use_generic_body_gate(&plugin));

    let mut entry = RequestFilterEntry::new(plugin);
    entry.conditions = Some(PluginConditions {
        skip: Some(ConditionSet::all_of(vec![Condition::KeyExist(KeyExistCondition {
            key: KeyGet::Header {
                name: "X-Bypass".to_string(),
            },
            negate: false,
            descriptor: Box::from(""),
        })])),
        run: None,
    });
    let mut runtime = PluginRuntime::new();
    let errors = runtime.add_from_request_filters(&[entry], "default", "default/policy");
    assert!(errors.iter().any(|error| error.contains("restriction config invalid")));
    assert!(runtime.has_invalid_plugins());
    runtime.seal_fail_closed();
    assert_eq!(
        runtime.request_plugins_iter().next().map(|filter| filter.name()),
        Some("RejectInvalidConfig")
    );
}
