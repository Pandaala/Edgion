//! Tests for resp.set_status() — setting response status code from DSL.

use super::*;

#[test]
fn test_resp_set_status_basic() {
    let source = r#"
        resp.set_status(503)
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_response_status()
        .withf(|s| *s == 503)
        .times(1)
        .returning(|_| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_set_status_conditional() {
    let source = r#"
        if resp.status == 502 {
            resp.set_status(503)
            resp.set_header("Retry-After", "30")
        }
    "#;
    let mut session = MockPluginSession::new();
    session.expect_get_response_status().returning(|| Some(502));
    session
        .expect_set_response_status()
        .withf(|s| *s == 503)
        .times(1)
        .returning(|_| Ok(()));
    session
        .expect_set_response_header()
        .withf(|n, v| n == "Retry-After" && v == "30")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_set_status_invalid_code() {
    let source = r#"resp.set_status(999)"#;
    let mut session = MockPluginSession::new();
    // Should get a runtime error for invalid status code
    session.expect_set_response_status().times(0);
    let result = run(source, session);
    // With Deny policy, runtime error → 500
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
}
