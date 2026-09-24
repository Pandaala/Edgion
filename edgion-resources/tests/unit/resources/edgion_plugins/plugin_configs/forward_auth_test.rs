use super::*;

/// New schema parses correctly: conn.target resolves port, request defaults
/// to forward_all=true, and validation passes.
#[test]
fn forward_auth_new_schema_parses() {
    let y = "target:\n  name: auth\n  port: 8080\nstatusOnError: 503\ndecision: {}\n";
    let cfg: ForwardAuthConfig = serde_yaml::from_str(y).unwrap();
    assert_eq!(cfg.conn.target.service.port, Some(8080));
    assert!(cfg.request.headers.forward_all); // forward_auth Default sets true
    assert!(cfg.validation_error().is_none());
}

/// Missing port must produce a validation error.
#[test]
fn forward_auth_rejects_missing_port() {
    let y = "target:\n  name: auth\ndecision: {}\n";
    let cfg: ForwardAuthConfig = serde_yaml::from_str(y).unwrap();
    assert!(cfg.validation_error().is_some());
}

/// statusOnError lives on the flattened conn, at the top level.
#[test]
fn forward_auth_failure_policy_is_top_level() {
    let y = "target:\n  name: auth\n  port: 8080\nstatusOnError: 502\ndecision: {}\n";
    let cfg: ForwardAuthConfig = serde_yaml::from_str(y).unwrap();
    assert_eq!(cfg.conn.status_on_error, 502);
    assert!(cfg.validation_error().is_none());
}

/// An unknown field under `decision:` must be rejected at deserialization
/// time, not silently dropped (`ForwardAuthDecision` denies unknown fields).
/// `allowDegradation` is used here as a representative now-removed field.
#[test]
fn forward_auth_rejects_stale_decision_allow_degradation() {
    let y = "target:\n  name: auth\n  port: 8080\ndecision:\n  allowDegradation: true\n";
    let err = serde_yaml::from_str::<ForwardAuthConfig>(y)
        .expect_err("stale decision.allowDegradation must fail to deserialize");
    assert!(
        err.to_string().contains("allowDegradation") || err.to_string().contains("unknown field"),
        "error should call out the unknown field; got: {err}"
    );
}

/// A stale `decision.statusOnError` (the pre-refactor nested location)
/// must be rejected at deserialization time, not silently dropped.
#[test]
fn forward_auth_rejects_stale_decision_status_on_error() {
    let y = "target:\n  name: auth\n  port: 8080\ndecision:\n  statusOnError: 500\n";
    let err = serde_yaml::from_str::<ForwardAuthConfig>(y)
        .expect_err("stale decision.statusOnError must fail to deserialize");
    assert!(
        err.to_string().contains("statusOnError") || err.to_string().contains("unknown field"),
        "error should call out the unknown field; got: {err}"
    );
}

/// statusOnError = 600 is out of range (200-599) and must be rejected.
#[test]
fn forward_auth_rejects_status_on_error_600() {
    let y = "target:\n  name: auth\n  port: 8080\nstatusOnError: 600\ndecision: {}\n";
    let cfg: ForwardAuthConfig = serde_yaml::from_str(y).unwrap();
    let err = cfg.validation_error().expect("600 should fail validation");
    assert!(
        err.contains("statusOnError"),
        "Error should mention statusOnError; got: {err}"
    );
}

/// Default::default() must set request.headers.forward_all = true (the
/// forward_auth default behavior contract).
#[test]
fn forward_auth_default_headers_forward_all_is_true() {
    let cfg = ForwardAuthConfig::default();
    assert!(
        cfg.request.headers.forward_all,
        "Default ForwardAuthConfig must have request.headers.forward_all = true"
    );
}

#[test]
fn forward_auth_default_passes_own_validation() {
    let cfg = ForwardAuthConfig::default();
    assert!(
        (200..=599).contains(&cfg.conn.status_on_error),
        "Default ForwardAuthConfig must pass status_on_error validation, got: {}",
        cfg.conn.status_on_error
    );
}

/// IMPORTANT 3 regression: `timeoutMs: 0` used to be admitted, and
/// `tokio::time::timeout(ZERO, ..)` fires instantly on every request,
/// making the auth call fail on (near-)every attempt. The check now lives
/// in the shared `validate_connection_target` (exercised here via
/// forward_auth's own validator, not just webhook's).
#[test]
fn forward_auth_rejects_zero_timeout_ms() {
    let y = "target:\n  name: auth\n  port: 8080\ntimeoutMs: 0\ndecision: {}\n";
    let cfg: ForwardAuthConfig = serde_yaml::from_str(y).unwrap();
    let err = cfg.validation_error().expect("timeoutMs: 0 must be rejected");
    assert!(err.contains("timeoutMs"), "got: {err}");
}

/// IMPORTANT 3 regression: `maxResponseBytes: 0` used to be admitted, and every
/// response then trips `ResponseTooLarge` -> Err, making the auth call fail
/// on (near-)every attempt.
#[test]
fn forward_auth_rejects_zero_max_response_bytes() {
    let y = "target:\n  name: auth\n  port: 8080\nmaxResponseBytes: 0\ndecision: {}\n";
    let cfg: ForwardAuthConfig = serde_yaml::from_str(y).unwrap();
    let err = cfg.validation_error().expect("maxResponseBytes: 0 must be rejected");
    assert!(err.contains("maxResponseBytes"), "got: {err}");
}

#[test]
fn forward_auth_inherits_body_content_type_conflict() {
    use crate::resources::common::outbound_request::{BodySpec, CustomField, CustomSource};
    let y = "target:\n  name: auth\n  port: 8080\nstatusOnError: 503\ndecision: {}\n";
    let mut cfg: ForwardAuthConfig = serde_yaml::from_str(y).unwrap();
    cfg.request.body = Some(BodySpec::Json {
        template: r#"{"user": "${ctx:user}"}"#.to_string(),
    });
    cfg.request.headers.custom.push(CustomField {
        name: "Content-Type".to_string(),
        source: CustomSource::Value("text/plain".to_string()),
    });
    let err = cfg
        .validation_error()
        .expect("forward_auth must inherit body validation");
    assert!(err.contains("Content-Type"), "got: {err}");
}

#[test]
fn forward_auth_rejects_conflicting_body_sources() {
    use crate::resources::common::outbound_request::BodySpec;

    let mut cfg = ForwardAuthConfig::default();
    cfg.conn.target.service.name = Some("auth".to_string());
    cfg.conn.target.service.port = Some(8080);
    cfg.forward_body = true;
    assert!(
        cfg.validation_error().is_none(),
        "forwardBody must remain valid when request.body is absent"
    );

    cfg.request.body = Some(BodySpec::Raw {
        template: "configured-body".to_string(),
    });

    let err = cfg
        .validation_error()
        .expect("forwardBody and request.body must be mutually exclusive");
    assert!(
        err.contains("forwardBody") && err.contains("request.body"),
        "got: {err}"
    );

    cfg.forward_body = false;
    assert!(
        cfg.validation_error().is_none(),
        "request.body must remain valid when forwardBody is false"
    );
}
