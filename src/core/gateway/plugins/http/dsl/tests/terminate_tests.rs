//! Tests for return terminate() — abrupt request termination without a response.

use super::*;

#[test]
fn test_return_terminate_basic() {
    let source = r#"return terminate()"#;
    let session = MockPluginSession::new();
    let result = run(source, session);
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
}

#[test]
fn test_return_terminate_with_space() {
    let source = "return terminate ()";
    let session = MockPluginSession::new();
    let result = run(source, session);
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
}

#[test]
fn test_return_terminate_conditional() {
    let source = r#"
        if req.header("X-Attack") != nil {
            return terminate()
        }
        return next()
    "#;
    // Case 1: header present → terminate
    let mut session = MockPluginSession::new();
    session
        .expect_header_value()
        .withf(|n| n == "X-Attack")
        .returning(|_| Some("true".to_string()));
    let result = run(source, session);
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);

    // Case 2: header absent → next
    let mut session = MockPluginSession::new();
    session
        .expect_header_value()
        .withf(|n| n == "X-Attack")
        .returning(|_| None);
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_return_terminate_with_log() {
    let source = r#"
        log("terminating request")
        return terminate()
    "#;
    let session = MockPluginSession::new();
    let vm = make_vm(source);
    let mut log = PluginLog::new("test");
    let mut s = session;
    let result = execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &DslErrorPolicy::Deny);
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert!(log.log.as_ref().unwrap().contains("terminating request"));
}
