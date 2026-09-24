use super::*;
use crate::conf_mgr::sync_runtime::resource_processor::status_utils::{
    condition_reasons, condition_true, condition_types,
};
use std::sync::Arc;

fn resource(paths: Vec<String>) -> LinkSys {
    let spec = serde_json::from_value(serde_json::json!({
        "type": "redis",
        "config": {"endpoints": ["redis://127.0.0.1:6379"]}
    }))
    .expect("minimal LinkSys spec");
    let mut resource = LinkSys::new("redis", spec);
    resource.metadata.namespace = Some("default".to_string());
    resource.metadata.generation = Some(7);
    resource.spec.unknown_fields = paths;
    resource
}

#[test]
fn update_status_adds_and_clears_advisory_without_disturbing_other_conditions() {
    let handler = LinkSysHandler::new();
    let mut resource = resource(vec!["config.timeout.conect".to_string()]);
    resource
        .status
        .get_or_insert_with(LinkSysStatus::default)
        .as_local_mut()
        .expect("status must be projected before handler mutation")
        .conditions
        .push(resolved_refs_condition(&[], Some(7)));
    let ctx = HandlerContext::new(
        None,
        None,
        Default::default(),
        5,
        20,
        Arc::new(crate::conf_mgr::ControllerRuntime::default()),
    );

    handler.update_status(&mut resource, &ctx, &[]);
    let conditions = &resource
        .status
        .as_ref()
        .expect("status")
        .as_local()
        .expect("native test status")
        .conditions;
    assert!(conditions.iter().any(|condition| {
        condition.type_ == condition_types::UNKNOWN_CONFIG_FIELDS
            && condition.reason == condition_reasons::UNKNOWN_FIELDS_DETECTED
            && condition.status == "True"
    }));
    assert!(conditions
        .iter()
        .any(|condition| condition.type_ == condition_types::ACCEPTED && condition.status == "True"));
    assert!(conditions
        .iter()
        .any(|condition| condition.type_ == condition_types::RESOLVED_REFS && condition.status == "True"));

    resource.spec.unknown_fields.clear();
    resource
        .status
        .as_mut()
        .expect("status")
        .as_local_mut()
        .expect("native test status")
        .conditions
        .push(condition_true("Unrelated", "Stable", "keep", Some(7)));
    handler.update_status(&mut resource, &ctx, &[]);
    let conditions = &resource
        .status
        .as_ref()
        .expect("status")
        .as_local()
        .expect("native test status")
        .conditions;
    assert!(conditions
        .iter()
        .all(|condition| condition.type_ != condition_types::UNKNOWN_CONFIG_FIELDS));
    assert!(conditions
        .iter()
        .any(|condition| condition.type_ == condition_types::RESOLVED_REFS && condition.status == "True"));
    assert!(conditions.iter().any(|condition| condition.type_ == "Unrelated"));
}
