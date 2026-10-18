//! Tests for request read-only builtins: headers, method, path, query, cookie, IPs, scheme, URI.

use super::*;

#[test]
fn borrowed_decoded_query_preserves_nil_empty_and_byte_limits() {
    for value in [None, Some(""), Some("é€")] {
        let length = value.map_or(0, str::len);
        for limit in [length, length.saturating_sub(1)] {
            let mut session = MockPluginSession::new();
            session.expect_get_query_param().times(0);
            session
                .expect_get_query_param_ref()
                .withf(|name| name == "item")
                .times(1)
                .return_const(value);
            let source = "let v = req.query(\"item\")\nif v == nil { req.set_header(\"X-Result\", \"missing\") } else { req.set_header(\"X-Result\", v) }";
            let limits = VmLimits {
                max_string_len: limit,
                ..Default::default()
            };
            if length > limit {
                assert!(
                    matches!(run_expect_runtime_err_with_limits(source, session, limits), RuntimeError::StringTooLong { len, limit: actual } if len == length && actual == limit)
                );
            } else {
                session
                    .expect_set_request_header()
                    .times(1)
                    .withf(move |name, actual| name == "X-Result" && actual == value.unwrap_or("missing"))
                    .returning(|_, _| Ok(()));
                assert_eq!(run_with_limits(source, session, limits), good_next());
            }
        }
    }
}

#[test]
fn borrowed_cookie_preserves_field_order_and_parser_rules() {
    for (fields, expected) in [
        (vec![], "missing"),
        (vec![b"item=".as_slice()], ""),
        (vec![b" other=x; item=a=b ".as_slice()], "a=b"),
        (vec![b"Item=wrong; item=first; item=second".as_slice()], "first"),
        (vec![b"other=x".as_slice(), b"item=second".as_slice()], "second"),
        (vec![b"\xff".as_slice(), b"item=second".as_slice()], "missing"),
        (vec![b"item=first".as_slice(), b"\xff".as_slice()], "first"),
    ] {
        let mut headers = http::HeaderMap::new();
        for field in fields {
            headers.append(http::header::COOKIE, http::HeaderValue::from_bytes(field).unwrap());
        }
        let mut session = MockPluginSession::new();
        session.expect_get_cookie().times(0);
        session.expect_request_header_map().times(1).return_const(headers);
        session
            .expect_set_request_header()
            .times(1)
            .withf(move |name, value| name == "X-Result" && value == expected)
            .returning(|_, _| Ok(()));
        assert_eq!(run(
            "let v = req.cookie(\"item\")\nif v == nil { req.set_header(\"X-Result\", \"missing\") } else { req.set_header(\"X-Result\", v) }",
            session,
        ), good_next());
    }
}

#[test]
fn borrowed_dynamic_header_preserves_first_text_value() {
    for (values, lookup, expected) in [
        (vec![], "X-VaLuE", "missing"),
        (vec![b"".as_slice()], "X-VaLuE", ""),
        (vec![b"first".as_slice(), b"second".as_slice()], "X-VaLuE", "first"),
        (vec![b"\xff".as_slice(), b"second".as_slice()], "X-VaLuE", "missing"),
        (vec![b"first".as_slice()], "bad name", "missing"),
    ] {
        let mut headers = http::HeaderMap::new();
        for value in values {
            headers.append("x-value", http::HeaderValue::from_bytes(value).unwrap());
        }
        let mut session = MockPluginSession::new();
        session.expect_header_value().times(0);
        session.expect_request_header_map().times(1).return_const(headers);
        session
            .expect_set_request_header()
            .times(1)
            .withf(move |name, value| name == "X-Result" && value == expected)
            .returning(|_, _| Ok(()));
        assert_eq!(run(
            &format!("let value = req.header(\"{lookup}\")\nif value == nil {{ req.set_header(\"X-Result\", \"missing\") }} else {{ req.set_header(\"X-Result\", value) }}"),
            session,
        ), good_next());
    }
}

#[test]
fn borrowed_raw_query_preserves_bytes_and_limits() {
    for query in [None, Some(""), Some("a=%2F&a=2+3&&empty=")] {
        let length = query.map_or(0, str::len);
        for limit in [length, length.saturating_sub(1)] {
            let mut session = MockPluginSession::new();
            session.expect_get_query().times(0);
            session.expect_request_query().times(1).return_const(query);
            let limits = VmLimits {
                max_string_len: limit,
                ..Default::default()
            };
            let source = "let value = req.query_string\nif value == nil { req.set_header(\"X-Result\", \"missing\") } else { req.set_header(\"X-Result\", value) }";
            if length > limit {
                assert!(
                    matches!(run_expect_runtime_err_with_limits(source, session, limits), RuntimeError::StringTooLong { len, limit: actual } if len == length && actual == limit)
                );
            } else {
                session
                    .expect_set_request_header()
                    .times(1)
                    .withf(move |name, value| name == "X-Result" && value == query.unwrap_or("missing"))
                    .returning(|_, _| Ok(()));
                assert_eq!(run_with_limits(source, session, limits), good_next());
            }
        }
    }
}

#[test]
fn borrowed_scheme_preserves_fallback_and_first_value() {
    for (forwarded, alternate, expected) in [
        (vec![], None, "http"),
        (vec![], Some("HTTPS"), "HTTPS"),
        (vec![b"".as_slice()], Some("https"), ""),
        (vec![b"CuStOm".as_slice()], Some("https"), "CuStOm"),
        (
            vec![b"\xff".as_slice(), b"https".as_slice()],
            Some("alternate"),
            "alternate",
        ),
        (vec![b"\xff".as_slice()], None, "http"),
        (vec![b"first".as_slice(), b"second".as_slice()], None, "first"),
    ] {
        let mut headers = http::HeaderMap::new();
        for value in forwarded {
            headers.append("x-forwarded-proto", http::HeaderValue::from_bytes(value).unwrap());
        }
        if let Some(value) = alternate {
            headers.insert("x-scheme", http::HeaderValue::from_static(value));
        }
        let mut session = MockPluginSession::new();
        session.expect_header_value().times(0);
        session.expect_request_header_map().return_const(headers);
        session
            .expect_set_request_header()
            .times(1)
            .withf(move |name, value| name == "X-Result" && value == expected)
            .returning(|_, _| Ok(()));
        assert_eq!(run("req.set_header(\"X-Result\", req.scheme)", session), good_next());
    }
}

#[test]
fn borrowed_request_scalar_limits_follow_value_selection() {
    for (expression, name) in [
        ("req.host", "host"),
        ("req.header(\"host\")", "host"),
        ("req.cookie(\"item\")", "cookie"),
        ("req.content_type", "content-type"),
        ("req.scheme", "x-forwarded-proto"),
    ] {
        for limit in [3, 4] {
            let mut headers = http::HeaderMap::new();
            headers.insert(
                http::HeaderName::from_static(name),
                http::HeaderValue::from_static(if name == "cookie" { "item=abcd" } else { "abcd" }),
            );
            headers.insert("x-scheme", http::HeaderValue::from_static("x"));
            let mut session = MockPluginSession::new();
            session.expect_header_value().times(0);
            session.expect_request_header_map().return_const(headers);
            let source = format!("let value = {expression}");
            let limits = VmLimits {
                max_string_len: limit,
                ..Default::default()
            };
            if limit == 3 {
                assert!(matches!(
                    run_expect_runtime_err_with_limits(&source, session, limits),
                    RuntimeError::StringTooLong { len: 4, limit: 3 }
                ));
            } else {
                assert_eq!(run_with_limits(&source, session, limits), good_next());
            }
        }
    }
}

#[test]
fn test_req_header_present() {
    let mut session = MockPluginSession::new();
    session.expect_header_value().times(0);
    session
        .expect_request_header_map()
        .return_const(http::HeaderMap::from_iter([(
            http::HeaderName::from_bytes(b"X-Foo").unwrap(),
            http::HeaderValue::from_static("bar"),
        )]));
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-Out" && v == "bar")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        "let v = req.header(\"X-Foo\")\nif v != nil { req.set_header(\"X-Out\", v) }",
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_req_header_absent() {
    let mut session = MockPluginSession::new();
    session.expect_header_value().times(0);
    session
        .expect_request_header_map()
        .return_const(http::HeaderMap::from_iter([]));
    let result = run(
        "let v = req.header(\"X-Foo\")\nif v != nil { req.set_header(\"X-Out\", v) }",
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_req_method() {
    let mut session = MockPluginSession::new();
    session.expect_get_method().return_const("POST".to_string());
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-M" && v == "POST")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(r#"req.set_header("X-M", req.method)"#, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_req_path() {
    let mut session = MockPluginSession::new();
    session.expect_get_path().return_const("/api/users".to_string());
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-P" && v == "/api/users")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(r#"req.set_header("X-P", req.path)"#, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_req_query() {
    let mut session = MockPluginSession::new();
    session.expect_get_query_param().times(0);
    session
        .expect_get_query_param_ref()
        .withf(|name| name == "page")
        .times(1)
        .return_const(Some("2"));
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-Q" && v == "2")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        "let q = req.query(\"page\")\nif q != nil { req.set_header(\"X-Q\", q) }",
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_req_query_string() {
    let mut session = MockPluginSession::new();
    session.expect_get_query().times(0);
    session.expect_request_query().return_const(Some("a=1&b=2"));
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-QS" && v == "a=1&b=2")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        "let qs = req.query_string\nif qs != nil { req.set_header(\"X-QS\", qs) }",
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_req_cookie() {
    let mut session = MockPluginSession::new();
    session.expect_get_cookie().times(0);
    session
        .expect_request_header_map()
        .times(1)
        .return_const(http::HeaderMap::from_iter([(
            http::header::COOKIE,
            http::HeaderValue::from_static("session=abc"),
        )]));
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-C" && v == "abc")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        "let c = req.cookie(\"session\")\nif c != nil { req.set_header(\"X-C\", c) }",
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_req_direct_peer_ip() {
    let mut session = MockPluginSession::new();
    session.expect_direct_peer_ip().return_const("1.2.3.4".to_string());
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-IP" && v == "1.2.3.4")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(r#"req.set_header("X-IP", req.direct_peer_ip)"#, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_req_remote_ip() {
    let mut session = MockPluginSession::new();
    session.expect_remote_ip().return_const("10.0.0.1".to_string());
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-RIP" && v == "10.0.0.1")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(r#"req.set_header("X-RIP", req.remote_ip)"#, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_req_host() {
    let mut session = MockPluginSession::new();
    session.expect_header_value().times(0);
    session
        .expect_request_header_map()
        .return_const(http::HeaderMap::from_iter([(
            http::header::HeaderName::from_bytes(b"Host").unwrap(),
            http::HeaderValue::from_static("example.com"),
        )]));
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-H" && v == "example.com")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run("let h = req.host\nif h != nil { req.set_header(\"X-H\", h) }", session);
    assert_eq!(result, good_next());
}

#[test]
fn test_req_scheme() {
    let mut session = MockPluginSession::new();
    session.expect_header_value().times(0);
    session
        .expect_request_header_map()
        .return_const(http::HeaderMap::from_iter([(
            http::header::HeaderName::from_bytes(b"X-Forwarded-Proto").unwrap(),
            http::HeaderValue::from_static("https"),
        )]));
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-S" && v == "https")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(r#"req.set_header("X-S", req.scheme)"#, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_req_uri() {
    let mut session = MockPluginSession::new();
    session.expect_get_uri().return_const("/api?x=1".to_string());
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-U" && v == "/api?x=1")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(r#"req.set_header("X-U", req.uri)"#, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_req_content_type() {
    let mut session = MockPluginSession::new();
    session.expect_header_value().times(0);
    session
        .expect_request_header_map()
        .return_const(http::HeaderMap::from_iter([(
            http::header::HeaderName::from_bytes(b"Content-Type").unwrap(),
            http::HeaderValue::from_static("application/json"),
        )]));
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-CT" && v == "application/json")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        "let ct = req.content_type\nif ct != nil { req.set_header(\"X-CT\", ct) }",
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_req_has_header_true() {
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("x-auth", "token")]);
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-R" && v == "yes")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        r#"if req.has_header("X-Auth") { req.set_header("X-R", "yes") }"#,
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_req_has_header_false() {
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[]);
    let result = run(
        r#"if req.has_header("X-Auth") { req.set_header("X-R", "yes") }"#,
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_req_header_names_for_in() {
    let mut session = MockPluginSession::new();
    setup_header_mocks(
        &mut session,
        &[("x-a", "v1"), ("x-b", "v2"), ("x-c", "v3"), ("x-a", "v4")],
    );
    for (name, count) in [("x-a", 2), ("x-b", 1), ("x-c", 1)] {
        session
            .expect_set_request_header()
            .withf(move |h, v| h == "X-Name" && v == name)
            .times(count)
            .returning(|_, _| Ok(()));
    }
    let result = run(
        r#"for name in req.header_names { req.set_header("X-Name", name) }"#,
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_req_header_names_truncates_field_lines_at_fixed_cap() {
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("x-a", "value"); 257]);
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-Count" && v == "256")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run_with_limits(
        r#"req.set_header("X-Count", len(req.header_names))"#,
        session,
        VmLimits {
            max_list_len: 1,
            ..Default::default()
        },
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_req_header_names_accepts_exact_name_byte_limit() {
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("x-a", "value")]);
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-Name" && v == "x-a")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run_with_limits(
        r#"for name in req.header_names { req.set_header("X-Name", name) }"#,
        session,
        VmLimits {
            max_string_len: 3,
            ..Default::default()
        },
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_req_header_names_rejects_name_over_byte_limit() {
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("x-a", "value")]);
    let err = run_expect_runtime_err_with_limits(
        "let names = req.header_names",
        session,
        VmLimits {
            max_string_len: 2,
            ..Default::default()
        },
    );
    assert!(matches!(err, RuntimeError::StringTooLong { len: 3, limit: 2 }));
}

#[test]
fn req_request_path_returns_entry_raw() {
    let mut session = MockPluginSession::new();
    session.expect_get_request_path().return_const("/%61dmin".to_string());
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-RP" && v == "/%61dmin")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(r#"req.set_header("X-RP", req.request_path)"#, session);
    assert_eq!(result, good_next());
}

#[test]
fn req_request_uri_returns_entry_raw() {
    let mut session = MockPluginSession::new();
    session
        .expect_get_request_uri()
        .return_const("/%61dmin?x=1".to_string());
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-RU" && v == "/%61dmin?x=1")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(r#"req.set_header("X-RU", req.request_uri)"#, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_req_path_param_present() {
    let mut session = MockPluginSession::new();
    session
        .expect_get_path_param()
        .withf(|k| k == "uid")
        .returning(|_| Some("789".to_string()));
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-Uid" && v == "789")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        "let u = req.path_param(\"uid\")\nif u != nil { req.set_header(\"X-Uid\", u) }",
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_req_path_param_absent() {
    let mut session = MockPluginSession::new();
    session
        .expect_get_path_param()
        .withf(|k| k == "missing")
        .returning(|_| None);
    let result = run(
        "let u = req.path_param(\"missing\")\nif u != nil { req.set_header(\"X-Uid\", u) }",
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn request_method_uses_entry_view_and_preserves_absence() {
    let mut session = MockPluginSession::new();
    session.expect_get_method().return_const("POST".to_string());
    session.expect_get_request_method().return_const(Some("GET"));
    assert_eq!(
        run(
            "if req.method != \"POST\" || req.request_method != \"GET\" { return deny(500, \"failed\") }",
            session
        ),
        good_next()
    );
    let mut session = MockPluginSession::new();
    session.expect_get_request_method().return_const(None);
    assert_eq!(
        run("if req.request_method != nil { return deny(500, \"failed\") }", session),
        good_next()
    );
}
