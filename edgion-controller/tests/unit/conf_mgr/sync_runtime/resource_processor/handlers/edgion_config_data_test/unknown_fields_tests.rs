use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::status_utils::{
    condition_reasons, condition_true, condition_types,
};
use edgion_resources::resources::edgion_config_data::EdgionConfigDataSpec;
use std::sync::Arc;

fn resource(paths: Vec<String>) -> EdgionConfigData {
    let spec: EdgionConfigDataSpec = serde_json::from_value(serde_json::json!({
        "data": {"type": "Misc", "config": {}}
    }))
    .expect("minimal ConfigData spec");
    let mut resource = EdgionConfigData::new("config", spec);
    resource.metadata.namespace = Some("default".to_string());
    resource.metadata.generation = Some(7);
    resource.spec.unknown_fields = paths;
    resource
}

#[test]
fn update_status_adds_and_clears_advisory_without_disturbing_other_conditions() {
    let handler = EdgionConfigDataHandler::new();
    let mut resource = resource(vec!["data.config.typo".to_string()]);
    let ctx = HandlerContext::new(
        None,
        None,
        Default::default(),
        5,
        20,
        Arc::new(crate::conf_mgr::ControllerRuntime::default()),
    );

    handler.update_status(&mut resource, &ctx, &[]);
    let conditions = &resource.status.as_ref().expect("status").conditions;
    assert!(conditions.iter().any(|condition| {
        condition.type_ == condition_types::UNKNOWN_CONFIG_FIELDS
            && condition.reason == condition_reasons::UNKNOWN_FIELDS_DETECTED
            && condition.status == "True"
    }));
    assert!(conditions
        .iter()
        .any(|condition| condition.type_ == condition_types::ACCEPTED && condition.status == "True"));

    resource.spec.unknown_fields.clear();
    resource
        .status
        .as_mut()
        .expect("status")
        .conditions
        .push(condition_true("Unrelated", "Stable", "keep", Some(7)));
    handler.update_status(&mut resource, &ctx, &[]);
    let conditions = &resource.status.as_ref().expect("status").conditions;
    assert!(conditions
        .iter()
        .all(|condition| condition.type_ != condition_types::UNKNOWN_CONFIG_FIELDS));
    assert!(conditions.iter().any(|condition| condition.type_ == "Unrelated"));
}

#[test]
fn proxy_protocol_unknown_config_field_sets_accepted_false() {
    let spec: EdgionConfigDataSpec = serde_json::from_value(serde_json::json!({
        "data": {"type": "ProxyProtocolTrust", "config": {"mode": "required", "typo": true}}
    }))
    .expect("security policy typo must remain deserializable");
    let mut resource = EdgionConfigData::new("proxy-policy", spec);
    resource.metadata.namespace = Some("default".to_string());
    resource.spec.unknown_fields = vec!["data.config.typo".to_string()];
    let handler = EdgionConfigDataHandler::new();
    let ctx = HandlerContext::new(
        None,
        None,
        Default::default(),
        5,
        20,
        Arc::new(crate::conf_mgr::ControllerRuntime::default()),
    );

    let errors = handler.preparse(&mut resource, &ctx);
    assert_eq!(errors, ["ProxyProtocolTrust invalid: policy validation failed"]);
    handler.update_status(&mut resource, &ctx, &errors);
    let accepted = resource
        .status
        .as_ref()
        .unwrap()
        .conditions
        .iter()
        .find(|condition| condition.type_ == condition_types::ACCEPTED)
        .unwrap();
    assert_eq!(accepted.status, "False");
    assert_eq!(accepted.reason, condition_reasons::INVALID);
}
