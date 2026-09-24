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
    for status in [103, 999] {
        let source = format!("resp.set_status({status})");
        let mut session = MockPluginSession::new();
        session.expect_set_response_status().times(0);
        let result = run(&source, session);
        assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
    }
}

#[test]
fn test_resp_set_status_accepts_endpoints_and_returns_nil() {
    for status in [200, 599] {
        let source = format!(
            r#"
            let result = resp.set_status({status})
            if result == nil {{
                resp.set_header("X-Status-Checked", "yes")
            }}
            "#
        );
        let mut session = MockPluginSession::new();
        session
            .expect_set_response_status()
            .withf(move |actual| *actual == status)
            .times(1)
            .returning(|_| Ok(()));
        session
            .expect_set_response_header()
            .withf(|name, value| name == "X-Status-Checked" && value == "yes")
            .times(1)
            .returning(|_, _| Ok(()));
        assert_eq!(run(&source, session), good_next(), "status {status}");
    }
}

#[test]
fn test_resp_set_status_invalid_integer_returns_api_error_before_host_call() {
    // The wide values would wrap to the valid status 200 with an unchecked u16 cast.
    for status in [103, 999, 199, 600, 65_736, -65_336] {
        let source = format!("resp.set_status({status})");
        let mut session = MockPluginSession::new();
        session.expect_set_response_status().times(0);
        let err = run_expect_runtime_err(&source, session);
        assert!(
            matches!(&err, RuntimeError::ApiError { function, .. } if function == "resp.set_status"),
            "status {status}: {err:?}"
        );
    }
}

#[test]
fn test_resp_set_status_non_integer_returns_type_error_before_host_call() {
    for (argument, expected_type) in [(r#""200""#, "Str"), ("nil", "Nil")] {
        let source = format!("resp.set_status({argument})");
        let mut session = MockPluginSession::new();
        session.expect_set_response_status().times(0);
        let err = run_expect_runtime_err(&source, session);
        assert!(
            matches!(
                &err,
                RuntimeError::TypeError {
                    expected: "Int",
                    got,
                    operation: "resp.set_status()",
                } if *got == expected_type
            ),
            "argument {argument}: {err:?}"
        );
    }
}
