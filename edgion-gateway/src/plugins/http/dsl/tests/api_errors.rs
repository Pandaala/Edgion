//! ApiError coverage for session-mutation builtins.
//!
//! `req.set_header` is covered by `runtime_errors.rs::test_api_error`; this file
//! adds the remaining mutation surfaces so every wrapper that calls
//! `.map_err(|e| api_error(..))` has at least one explicit test.

use super::*;

fn err_inject() -> crate::plugins::runtime::traits::session::PluginSessionError {
    Box::new(std::io::Error::other("injected session failure"))
}

fn assert_api_error_function(error: &RuntimeError, expected: &'static str) {
    match error {
        RuntimeError::ApiError { function, .. } => {
            assert_eq!(function, expected, "unexpected ApiError function")
        }
        other => panic!("expected ApiError({expected}), got {other:?}"),
    }
}

#[test]
fn test_api_error_req_set_uri() {
    let mut session = MockPluginSession::new();
    session
        .expect_set_upstream_uri()
        .withf(|u| u == "/v2")
        .times(1)
        .returning(|_| Err(err_inject()));
    let err = run_expect_runtime_err(r#"req.set_uri("/v2")"#, session);
    assert_api_error_function(&err, "req.set_uri");
}

#[test]
fn test_api_error_req_set_host() {
    let mut session = MockPluginSession::new();
    session
        .expect_set_upstream_host()
        .withf(|h| h == "example.test")
        .times(1)
        .returning(|_| Err(err_inject()));
    let err = run_expect_runtime_err(r#"req.set_host("example.test")"#, session);
    assert_api_error_function(&err, "req.set_host");
}

#[test]
fn test_api_error_req_set_method() {
    let mut session = MockPluginSession::new();
    session
        .expect_set_upstream_method()
        .withf(|m| m == "POST")
        .times(1)
        .returning(|_| Err(err_inject()));
    let err = run_expect_runtime_err(r#"req.set_method("POST")"#, session);
    assert_api_error_function(&err, "req.set_method");
}

#[test]
fn test_api_error_resp_set_status() {
    let mut session = MockPluginSession::new();
    session
        .expect_set_response_status()
        .withf(|s| *s == 200u16)
        .times(1)
        .returning(|_| Err(err_inject()));
    let err = run_expect_runtime_err(r#"resp.set_status(200)"#, session);
    assert_api_error_function(&err, "resp.set_status");
}

#[test]
fn test_api_error_req_append_header() {
    let mut session = MockPluginSession::new();
    session
        .expect_append_request_header()
        .withf(|name, value| name == "X-Trace" && value == "second")
        .times(1)
        .returning(|_, _| Err(err_inject()));
    let err = run_expect_runtime_err(r#"req.append_header("X-Trace", "second")"#, session);
    assert_api_error_function(&err, "req.append_header");
}

#[test]
fn test_api_error_req_remove_header() {
    let mut session = MockPluginSession::new();
    session
        .expect_remove_request_header()
        .withf(|name| name == "X-Internal")
        .times(1)
        .returning(|_| Err(err_inject()));
    let err = run_expect_runtime_err(r#"req.remove_header("X-Internal")"#, session);
    assert_api_error_function(&err, "req.remove_header");
}

#[test]
fn test_api_error_resp_set_header() {
    let mut session = MockPluginSession::new();
    session
        .expect_set_response_header()
        .withf(|name, value| name == "Cache-Control" && value == "no-store")
        .times(1)
        .returning(|_, _| Err(err_inject()));
    let err = run_expect_runtime_err(r#"resp.set_header("Cache-Control", "no-store")"#, session);
    assert_api_error_function(&err, "resp.set_header");
}

#[test]
fn test_api_error_resp_append_header() {
    let mut session = MockPluginSession::new();
    session
        .expect_append_response_header()
        .withf(|name, value| name == "Vary" && value == "Origin")
        .times(1)
        .returning(|_, _| Err(err_inject()));
    let err = run_expect_runtime_err(r#"resp.append_header("Vary", "Origin")"#, session);
    assert_api_error_function(&err, "resp.append_header");
}

#[test]
fn test_api_error_resp_remove_header() {
    let mut session = MockPluginSession::new();
    session
        .expect_remove_response_header()
        .withf(|name| name == "Server")
        .times(1)
        .returning(|_| Err(err_inject()));
    let err = run_expect_runtime_err(r#"resp.remove_header("Server")"#, session);
    assert_api_error_function(&err, "resp.remove_header");
}
