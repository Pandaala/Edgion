//! Tests for return terminate() — an explicit HTTP 502 local response with no body.

use super::*;

#[test]
fn test_return_terminate_basic() {
    let source = r#"return terminate()"#;
    let session = MockPluginSession::new();
    let result = run(source, session);
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 502,
            body: None
        }
    );
}

#[test]
fn test_return_terminate_with_space() {
    let source = "return terminate ()";
    let session = MockPluginSession::new();
    let result = run(source, session);
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 502,
            body: None
        }
    );
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
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 502,
            body: None
        }
    );

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
    let vm = make_vm(source);
    let mut log = PluginLog::new("test");
    let mut s = MockPluginSession::new();
    let rt_ctx = DslRuntime::default();
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(execute_safe(
            &vm,
            &mut s as &mut dyn PluginSession,
            &mut log,
            &DslErrorPolicy::Deny,
            &rt_ctx,
        ));
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 502,
            body: None
        }
    );
    assert!(log.log.as_ref().unwrap().contains("terminating request"));
}

#[test]
fn test_return_terminate_is_not_overridden_by_error_policy() {
    let vm = make_vm("return terminate()");
    for policy in [
        DslErrorPolicy::Ignore,
        DslErrorPolicy::Deny,
        DslErrorPolicy::DenyWith {
            status: 403,
            body: Some("policy error".to_string()),
        },
    ] {
        let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &policy);
        assert_eq!(
            result,
            PluginRunningResult::ErrResponse {
                status: 502,
                body: None
            }
        );
        assert_eq!(log.render_for_test(), "Deny 502");
    }
}
