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
    let mut headers = http::HeaderMap::new();
    headers.append("content-type", http::HeaderValue::from_static("application/json"));
    headers.append("content-type", http::HeaderValue::from_static("text/plain"));
    headers.append("x-custom", http::HeaderValue::from_bytes(b"\xff").unwrap());
    let expected_names: Vec<_> = headers.keys().map(|name| name.as_str().to_owned()).collect();
    session.expect_response_header_names().times(0);
    session.expect_response_header_map().times(1).return_const(headers);
    let mut sequence = mockall::Sequence::new();
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Count" && v == "2")
        .times(1)
        .returning(|_, _| Ok(()));
    for name in expected_names {
        session
            .expect_set_request_header()
            .withf(move |n, v| n == "X-Name" && v == name)
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(()));
    }
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_header_names_retains_fixed_prefix_at_exact_name_byte_limit() {
    let mut session = MockPluginSession::new();
    let mut headers = http::HeaderMap::new();
    for i in 0..256 {
        headers.insert(
            http::header::HeaderName::from_bytes(format!("x{i:03}").as_bytes()).unwrap(),
            http::HeaderValue::from_static("v"),
        );
    }
    headers.insert("x-oversized-tail", http::HeaderValue::from_static("v"));
    assert_eq!(headers.keys().nth(256).unwrap().as_str(), "x-oversized-tail");
    session.expect_response_header_names().times(0);
    session.expect_response_header_map().times(1).return_const(headers);
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
    session.expect_response_header_names().times(0);
    session
        .expect_response_header_map()
        .times(1)
        .return_const(http::HeaderMap::from_iter([(
            http::header::HeaderName::from_static("x000"),
            http::HeaderValue::from_static("v"),
        )]));
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
    session.expect_response_header_names().times(0);
    session
        .expect_response_header_map()
        .times(1)
        .return_const(http::HeaderMap::new());
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Empty" && v == "true")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}
