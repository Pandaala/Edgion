use super::*;
use crate::resources::edgion_plugins::validate::applicable_stages_by_name;

fn parse(body: &str) -> AiGuardConfig {
    yaml_serde::from_str(body).expect("configuration parses")
}

#[test]
fn plugin_variant_is_request_only() {
    let config = parse(
        r#"
rules:
  - name: block-key
    match:
      type: pii
      detector: awsAccessKeyId
    action: reject
"#,
    );
    assert!(config.validation_error().is_none());
    let stages = applicable_stages_by_name("AiGuard");
    assert!(stages.request);
    assert!(!stages.upstream_response_filter);
    assert!(!stages.upstream_response_body_filter);
    assert!(!stages.upstream_response);
}

#[test]
fn validation_error_returns_field_error_without_validate_call() {
    let config = AiGuardConfig::default();
    assert!(config.validation_error().is_some());
}

#[test]
fn rejects_bad_patterns_duplicate_names_and_action_mismatch() {
    let unclosed = parse(
        r#"
rules:
  - name: bad
    match:
      type: regex
      pattern: "a("
"#,
    );
    assert!(unclosed.validation_error().is_some());

    let duplicate = parse(
        r#"
rules:
  - name: same
    match: { type: regex, pattern: "a" }
  - name: same
    match: { type: regex, pattern: "b" }
"#,
    );
    assert!(duplicate
        .validation_error()
        .is_some_and(|error| error.contains("duplicate rule name")));

    let mask_without_replacement = parse(
        r#"
rules:
  - name: m
    match: { type: regex, pattern: "a" }
    action: mask
"#,
    );
    assert!(mask_without_replacement
        .validation_error()
        .is_some_and(|error| error.contains("replacement is required")));

    let replacement_on_reject = parse(
        r#"
rules:
  - name: r
    match: { type: regex, pattern: "a" }
    action: reject
    replacement: "x"
"#,
    );
    assert!(replacement_on_reject
        .validation_error()
        .is_some_and(|error| error.contains("only valid for action mask")));
}

#[test]
fn rejects_out_of_range_bounds() {
    let mut config = parse(
        r#"
rules:
  - name: a
    match: { type: regex, pattern: "a" }
"#,
    );
    config.max_spans = AI_GUARD_MAX_SPANS + 1;
    assert!(config.validation_error().is_some_and(|e| e.contains("maxSpans")));

    let mut config = parse(
        r#"
rules:
  - name: a
    match: { type: regex, pattern: "a" }
"#,
    );
    config.max_span_bytes = Some(4096);
    config.max_total_span_bytes = Some(1024);
    assert!(config
        .validation_error()
        .is_some_and(|e| e.contains("at least maxSpanBytes")));
}

#[test]
fn builtin_detectors_compile_under_the_shared_limits() {
    for detector in [
        AiGuardPiiDetector::Email,
        AiGuardPiiDetector::IpAddress,
        AiGuardPiiDetector::CreditCard,
        AiGuardPiiDetector::UsSocialSecurityNumber,
        AiGuardPiiDetector::AwsAccessKeyId,
    ] {
        assert!(
            compile_guard_pattern(detector.pattern()).is_ok(),
            "detector pattern must compile"
        );
    }
}

#[test]
fn evaluator_rejects_replay_and_operator_owned_protocol_fields() {
    let base = r#"
rules:
  - name: a
    match: { type: regex, pattern: "a" }
evaluator:
  target:
    url: https://guard.example
  timeoutMs: 1000
  request:
    path:
      template: /v1/decision
"#;
    assert!(parse(base).validation_error().is_none());

    let with_retry = parse(&format!("{base}  retry:\n    maxRetries: 2\n"));
    assert!(with_retry
        .validation_error()
        .is_some_and(|error| error.contains("retry is not supported")));
}

#[test]
fn resolved_secrets_are_absent_from_schema() {
    let schema = serde_json::to_string(&schemars::schema_for!(AiGuardConfig)).expect("schema serializes");
    assert!(!schema.contains("resolvedSecrets"));
}
