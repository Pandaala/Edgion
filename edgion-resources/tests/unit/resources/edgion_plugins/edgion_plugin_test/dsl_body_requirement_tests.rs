use super::*;
use crate::resources::edgion_plugins::body_requirement::BodyReadFailureMode;
use crate::resources::edgion_plugins::plugin_configs::{DslConfig, DslErrorPolicy};

fn dsl(source: &str, policy: Option<DslErrorPolicy>) -> EdgionPlugin {
    EdgionPlugin::Dsl(DslConfig {
        name: "t".into(),
        source: Some(source.into()),
        error_policy: policy,
        ..Default::default()
    })
}

#[test]
fn dsl_req_body_gated_at_request_stage_only() {
    let p = dsl("let b = req.body", None);
    assert!(p.body_requirement(BodyStage::Request).is_some());
    assert!(p.body_requirement(BodyStage::Response).is_none());
}

#[test]
fn dsl_resp_body_is_rejected_instead_of_gated() {
    let p = dsl("let b = resp.body", None);
    let EdgionPlugin::Dsl(config) = &p else { unreachable!() };
    assert!(config.compile_and_validate().unwrap_err().contains("streaming-only"));
    assert!(p.body_requirement(BodyStage::Response).is_none());
    assert!(p.body_requirement(BodyStage::Request).is_none());
}

#[test]
fn dsl_header_only_never_gated() {
    let p = dsl("let h = req.header(\"X\")", None);
    assert!(p.body_requirement(BodyStage::Request).is_none());
    assert!(p.body_requirement(BodyStage::Response).is_none());
}

#[test]
fn dsl_failure_mode_is_fail_close_regardless_of_error_policy() {
    // `errorPolicy` governs script RUNTIME errors, not body availability. A
    // refusal or a failed read means the script's body check never ran, so
    // the intrinsic body requirement is unconditionally fail-close — this
    // used to vary with `errorPolicy` (None/Ignore => fail-open), which is
    // exactly the silent-pass hole this test now guards against.
    let none_policy = dsl("let b = req.body", None);
    assert_eq!(
        none_policy
            .body_requirement(BodyStage::Request)
            .unwrap()
            .on_read_failure,
        BodyReadFailureMode::FailClose
    );
    let deny = dsl("let b = req.body", Some(DslErrorPolicy::Deny));
    assert_eq!(
        deny.body_requirement(BodyStage::Request).unwrap().on_read_failure,
        BodyReadFailureMode::FailClose
    );
}

#[test]
fn dsl_body_requirement_is_fail_close_regardless_of_error_policy() {
    // `errorPolicy` governs SCRIPT runtime errors. Body availability is not a
    // script error: a refusal or a failed read means the script's body check
    // never ran, and an enforcement script must not silently pass.
    for policy in [r#""ignore""#, r#""deny""#] {
        let json = format!(
            r#"{{ "type": "Dsl", "config": {{ "name": "t", "source": "let b = req.body", "errorPolicy": {policy} }} }}"#
        );
        let p: EdgionPlugin = serde_json::from_str(&json).expect("parses");
        let req = p
            .body_requirement(BodyStage::Request)
            .expect("dsl script referencing req.body declares a body requirement");
        assert_eq!(
            req.on_read_failure,
            BodyReadFailureMode::FailClose,
            "errorPolicy {policy} must not weaken the body requirement"
        );
    }
}

#[test]
fn strict_json_string_reads_are_not_gated_but_publication_is() {
    assert!(dsl("json.read(\"{}\")", None)
        .body_requirement(BodyStage::Request)
        .is_none());
    for policy in [None, Some(DslErrorPolicy::Ignore)] {
        let plugin = dsl("json.set_body(\"{}\")", policy);
        assert_eq!(
            plugin.body_requirement(BodyStage::Request).unwrap().on_read_failure,
            BodyReadFailureMode::FailClose
        );
        assert!(plugin.body_requirement(BodyStage::Response).is_none());
        let EdgionPlugin::Dsl(config) = plugin else {
            unreachable!()
        };
        let mut wire = config.clone();
        wire.bytecode = Some(config.compile_and_validate().unwrap().serialize_base64().unwrap());
        wire.source = None;
        assert!(EdgionPlugin::Dsl(wire).body_requirement(BodyStage::Request).is_some());
    }
}
