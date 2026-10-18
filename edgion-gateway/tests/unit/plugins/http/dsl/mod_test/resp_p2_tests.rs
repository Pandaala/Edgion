//! Tests for P2 response builtins: resp.content_type, resp.header_names().

use super::*;

#[test]
fn test_resp_content_type() {
    let source = r#"
        let ct = resp.content_type
        if ct != nil {
            req.set_header("X-CT", ct)
        }
    "#;
    let mut session = MockPluginSession::new();
    session.expect_get_response_header().times(0);
    session
        .expect_response_header_map()
        .return_const(http::HeaderMap::from_iter([(
            http::header::HeaderName::from_bytes(b"Content-Type").unwrap(),
            http::HeaderValue::from_static("application/json"),
        )]));
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-CT" && v == "application/json")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_content_type_missing_returns_nil() {
    let mut session = MockPluginSession::new();
    session.expect_get_response_header().times(0);
    session
        .expect_response_header_map()
        .return_const(http::HeaderMap::from_iter([]));
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Missing" && v == "true")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        r#"if resp.content_type == nil { req.set_header("X-Missing", "true") }"#,
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_header_names() {
    let source = r#"
        let names = resp.header_names()
        req.set_header("X-Count", to_str(len(names)))
        for name in names {
            req.set_header("X-Name", name)
        }
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_response_header_names()
        .returning(|| vec!["content-type".to_string(), "x-custom".to_string()]);
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Count" && v == "2")
        .times(1)
        .returning(|_, _| Ok(()));
    for name in ["content-type", "x-custom"] {
        session
            .expect_set_request_header()
            .withf(move |n, v| n == "X-Name" && v == name)
            .times(1)
            .returning(|_, _| Ok(()));
    }
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_header_names_retains_fixed_prefix_at_exact_name_byte_limit() {
    let mut session = MockPluginSession::new();
    session.expect_response_header_names().times(1).returning(|| {
        let mut names: Vec<_> = (0..256).map(|i| format!("x{i:03}")).collect();
        names.push("x-oversized-tail".to_string());
        names
    });
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Count" && v == "256")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run_with_limits(
        r#"req.set_header("X-Count", len(resp.header_names()))"#,
        session,
        VmLimits {
            max_list_len: 1,
            max_string_len: 4,
            ..Default::default()
        },
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_header_names_rejects_retained_name_over_byte_limit() {
    let mut session = MockPluginSession::new();
    session
        .expect_response_header_names()
        .times(1)
        .returning(|| vec!["x000".to_string()]);
    let err = run_expect_runtime_err_with_limits(
        "let names = resp.header_names()",
        session,
        VmLimits {
            max_string_len: 3,
            ..Default::default()
        },
    );
    assert!(matches!(err, RuntimeError::StringTooLong { len: 4, limit: 3 }));
}

#[test]
fn test_resp_header_names_empty() {
    let source = r#"
        let names = resp.header_names()
        if len(names) == 0 {
            req.set_header("X-Empty", "true")
        }
    "#;
    let mut session = MockPluginSession::new();
    session.expect_response_header_names().returning(Vec::new);
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Empty" && v == "true")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}
