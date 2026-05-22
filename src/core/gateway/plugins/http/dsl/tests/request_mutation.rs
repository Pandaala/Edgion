//! Tests for request mutation: set/append/remove header, set URI/host/method.

use super::*;

#[test]
fn test_set_header() {
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-A" && v == "v1")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(r#"req.set_header("X-A", "v1")"#, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_append_header() {
    let mut session = MockPluginSession::new();
    session
        .expect_append_request_header()
        .withf(|h, v| h == "X-A" && v == "v2")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(r#"req.append_header("X-A", "v2")"#, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_remove_header() {
    let mut session = MockPluginSession::new();
    session
        .expect_remove_request_header()
        .withf(|h| h == "X-Del")
        .times(1)
        .returning(|_| Ok(()));
    let result = run(r#"req.remove_header("X-Del")"#, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_set_uri() {
    let mut session = MockPluginSession::new();
    session
        .expect_set_upstream_uri()
        .withf(|u| u == "/new/path")
        .times(1)
        .returning(|_| Ok(()));
    let result = run(r#"req.set_uri("/new/path")"#, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_set_host() {
    let mut session = MockPluginSession::new();
    session
        .expect_set_upstream_host()
        .withf(|h| h == "new.host")
        .times(1)
        .returning(|_| Ok(()));
    let result = run(r#"req.set_host("new.host")"#, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_set_method() {
    let mut session = MockPluginSession::new();
    session
        .expect_set_upstream_method()
        .withf(|m| m == "POST")
        .times(1)
        .returning(|_| Ok(()));
    let result = run(r#"req.set_method("POST")"#, session);
    assert_eq!(result, good_next());
}
