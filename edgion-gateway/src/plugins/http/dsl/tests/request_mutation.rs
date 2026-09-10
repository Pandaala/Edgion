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
    let result = run(
        r#"let result = req.set_header("X-A", "v1")
           if result != nil { return deny(403, nil) }"#,
        session,
    );
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
    let result = run(
        r#"let result = req.append_header("X-A", "v2")
           if result != nil { return deny(403, nil) }"#,
        session,
    );
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
    let result = run(
        r#"let result = req.remove_header("X-Del")
           if result != nil { return deny(403, nil) }"#,
        session,
    );
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
    let result = run(
        r#"let result = req.set_uri("/new/path")
           if result != nil { return deny(403, nil) }"#,
        session,
    );
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
    let result = run(
        r#"let result = req.set_host("new.host")
           if result != nil { return deny(403, nil) }"#,
        session,
    );
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
    let result = run(
        r#"let result = req.set_method("POST")
           if result != nil { return deny(403, nil) }"#,
        session,
    );
    assert_eq!(result, good_next());
}
