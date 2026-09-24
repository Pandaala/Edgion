use super::*;

#[test]
fn default_is_fail_close() {
    let req = BodyRequirement::default();
    assert!(req.is_fail_close());
    assert_eq!(req.on_read_failure, BodyReadFailureMode::FailClose);
    assert!(req.max_body_size.is_none());
    assert!(req.conditions.is_none());
}

#[test]
fn deserializes_camel_case_and_fail_open() {
    let req: BodyRequirement = serde_json::from_str(r#"{ "maxBodySize": "1m", "onReadFailure": "failOpen" }"#).unwrap();
    assert_eq!(req.max_body_size.as_deref(), Some("1m"));
    assert_eq!(req.max_body_size_bytes(), Some(Ok(1024 * 1024)));
    assert_eq!(req.on_read_failure, BodyReadFailureMode::FailOpen);
    assert!(!req.is_fail_close());
}

#[test]
fn empty_object_defaults_fail_close() {
    let req: BodyRequirement = serde_json::from_str("{}").unwrap();
    assert!(req.is_fail_close());
    assert!(req.max_body_size_bytes().is_none());
    assert!(req.validation_error().is_none());
}

#[test]
fn invalid_and_zero_max_body_size_are_validation_errors() {
    let bad: BodyRequirement = serde_json::from_str(r#"{ "maxBodySize": "notasize" }"#).unwrap();
    assert!(bad.validation_error().is_some());
    let zero: BodyRequirement = serde_json::from_str(r#"{ "maxBodySize": "0" }"#).unwrap();
    assert!(zero.validation_error().unwrap().contains("greater than 0"));
}

#[test]
fn secondary_conditions_parse() {
    let req: BodyRequirement = serde_json::from_str(
            r#"{ "conditions": { "run": { "allOf": [ { "type": "keyMatch", "key": { "type": "method" }, "values": ["POST"] } ] } } }"#,
        )
        .unwrap();
    assert!(req.conditions.is_some());
}

#[test]
fn resolve_operator_cannot_downgrade_intrinsic_fail_close() {
    // Intrinsic fail-close (e.g. HMAC body validation) + operator failOpen →
    // stays fail-close (security plugin cannot be silently bypassed), but the
    // operator's maxSize tuning is still honored.
    let operator = BodyRequirement {
        max_body_size: Some("2m".to_string()),
        on_read_failure: BodyReadFailureMode::FailOpen,
        ..Default::default()
    };
    let intrinsic = BodyRequirement::fail_close();
    let resolved = BodyRequirement::resolve(Some(operator), Some(intrinsic)).unwrap();
    assert!(resolved.is_fail_close());
    assert_eq!(resolved.max_body_size.as_deref(), Some("2m"));

    // Intrinsic fail-open + operator fail-close → operator can tighten.
    let operator = BodyRequirement::fail_close();
    let intrinsic = BodyRequirement {
        on_read_failure: BodyReadFailureMode::FailOpen,
        ..Default::default()
    };
    let resolved = BodyRequirement::resolve(Some(operator), Some(intrinsic)).unwrap();
    assert!(resolved.is_fail_close());

    // No operator, intrinsic only → intrinsic used as-is.
    let resolved = BodyRequirement::resolve(None, Some(BodyRequirement::fail_close())).unwrap();
    assert!(resolved.is_fail_close());
    // Neither → None.
    assert!(BodyRequirement::resolve(None, None).is_none());

    // Operator body: on a plugin that does NOT consume body (intrinsic None)
    // → None: the operator cannot create a requirement, so a non-body plugin
    // is never gated (no spurious 500).
    let operator = BodyRequirement {
        max_body_size: Some("1m".to_string()),
        ..Default::default()
    };
    assert!(BodyRequirement::resolve(Some(operator), None).is_none());

    // Operator cannot narrow enforcement SCOPE of a fail-close, always-needs-
    // body plugin (no intrinsic conditions): operator conditions are dropped,
    // but size tuning survives.
    let operator = BodyRequirement {
        max_body_size: Some("2m".to_string()),
        conditions: Some(PluginConditions {
            skip: None,
            run: Some(crate::resources::edgion_plugins::conditions::ConditionSet::all_of(
                vec![],
            )),
        }),
        on_read_failure: BodyReadFailureMode::FailOpen,
    };
    let resolved = BodyRequirement::resolve(Some(operator), Some(BodyRequirement::fail_close())).unwrap();
    assert!(resolved.is_fail_close());
    assert!(resolved.conditions.is_none(), "operator conditions must be dropped");
    assert_eq!(resolved.max_body_size.as_deref(), Some("2m"));
}

#[test]
fn resolve_inherits_intrinsic_max_when_operator_omits_it() {
    let intrinsic = BodyRequirement {
        max_body_size: Some("64KiB".to_string()),
        conditions: Some(PluginConditions {
            skip: None,
            run: Some(crate::resources::edgion_plugins::conditions::ConditionSet::all_of(
                vec![],
            )),
        }),
        ..Default::default()
    };

    let empty_operator: BodyRequirement = serde_json::from_str("{}").unwrap();
    let resolved = BodyRequirement::resolve(Some(empty_operator), Some(intrinsic.clone())).unwrap();
    assert_eq!(resolved.max_body_size.as_deref(), Some("64KiB"));
    assert!(resolved.conditions.is_some());

    let policy_only_operator: BodyRequirement = serde_json::from_str(r#"{ "onReadFailure": "failOpen" }"#).unwrap();
    let resolved = BodyRequirement::resolve(Some(policy_only_operator), Some(intrinsic)).unwrap();
    assert_eq!(resolved.max_body_size.as_deref(), Some("64KiB"));
    assert!(resolved.conditions.is_some());
    assert!(resolved.is_fail_close());
}

#[test]
fn max_body_size_bytes_parses_the_limit() {
    let req: BodyRequirement = serde_json::from_str(r#"{ "maxBodySize": "64M" }"#).unwrap();
    assert_eq!(req.max_body_size_bytes(), Some(Ok(64 * 1024 * 1024)));
}
