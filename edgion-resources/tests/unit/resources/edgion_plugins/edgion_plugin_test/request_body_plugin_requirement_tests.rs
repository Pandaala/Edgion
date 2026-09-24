use super::*;
use crate::conditions::PluginConditions;
use crate::resources::edgion_plugins::{BodyReadFailureMode, BodyRequirement};

#[test]
fn request_body_features_declare_request_only_requirements() {
    for json in [
        r#"{"type":"ForwardAuth","config":{"target":{"name":"auth","port":80},"forwardBody":true,"decision":{}}}"#,
        r#"{"type":"ProxyRewrite","config":{"jsonBody":{"operations":[{"op":"replace","path":"/name","value":"gateway"}]}}}"#,
        r#"{"type":"ExtProc","config":{"grpcService":{"target":"processor:9000"},"processingMode":{"requestBodyMode":"BUFFERED"}}}"#,
        r#"{"type":"RequestBodyBuffer","config":{}}"#,
        r#"{"type":"FormJsonTransform","config":{"direction":"formToJson"}}"#,
    ] {
        let plugin: EdgionPlugin = serde_json::from_str(json).expect("body plugin config parses");
        assert!(plugin.body_requirement(BodyStage::Request).is_some(), "{json}");
        assert!(plugin.body_requirement(BodyStage::Response).is_none(), "{json}");
    }
}

#[test]
fn ext_proc_override_reductions_do_not_change_static_body_acquisition() {
    let buffered: EdgionPlugin = serde_json::from_str(
            r#"{"type":"ExtProc","config":{"grpcService":{"target":"processor:9000"},"processingMode":{"requestBodyMode":"BUFFERED"},"allowModeOverride":true,"allowedOverrideModes":[{"requestBodyMode":"NONE"}]}}"#,
        )
        .unwrap();
    assert!(buffered.body_requirement(BodyStage::Request).is_some());

    let streamed: EdgionPlugin = serde_json::from_str(
            r#"{"type":"ExtProc","config":{"grpcService":{"target":"processor:9000"},"processingMode":{"requestBodyMode":"STREAMED"},"allowModeOverride":true,"allowedOverrideModes":[{"requestBodyMode":"NONE"}]}}"#,
        )
        .unwrap();
    assert!(streamed.body_requirement(BodyStage::Request).is_none());
}

#[test]
fn proxy_rewrite_json_body_defaults_to_64_kib_fail_close() {
    let plugin: EdgionPlugin = serde_json::from_str(
        r#"{"type":"ProxyRewrite","config":{"jsonBody":{"operations":[{"op":"add","path":"/enabled","value":true}]}}}"#,
    )
    .unwrap();
    let requirement = plugin.body_requirement(BodyStage::Request).unwrap();
    assert_eq!(requirement.max_body_size_bytes(), Some(Ok(DEFAULT_INSTANCE_MAX_BYTES)));
    assert!(requirement.is_fail_close());
    assert!(requirement.conditions.is_none());
}

#[test]
fn request_body_buffer_preserves_operator_capture_conditions() {
    let plugin: EdgionPlugin = serde_json::from_str(r#"{"type":"RequestBodyBuffer","config":{}}"#).unwrap();
    let operator = BodyRequirement {
        conditions: Some(PluginConditions {
            skip: None,
            run: Some(crate::ConditionSet::all_of(vec![])),
        }),
        on_read_failure: BodyReadFailureMode::FailOpen,
        ..Default::default()
    };

    let resolved = BodyRequirement::resolve(Some(operator), plugin.body_requirement(BodyStage::Request)).unwrap();
    assert!(resolved.conditions.is_some());
    assert!(resolved.is_fail_close());
}
