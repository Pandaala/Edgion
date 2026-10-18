//! Tests for response builtins: status read/write, header read/write, upstream response filter phase.

use super::*;

#[test]
fn borrowed_response_reads_preserve_first_text_value() {
    for (values, expected) in [
        (vec![], None),
        (vec![b"".as_slice()], Some("")),
        (vec![b"first".as_slice(), b"second".as_slice()], Some("first")),
        (vec![b"\xff".as_slice(), b"second".as_slice()], None),
        (vec![b"first".as_slice(), b"\xff".as_slice()], Some("first")),
    ] {
        for expression in [
            "resp.header(\"CoNtEnT-TyPe\")",
            "resp.content_type",
            "resp.has_header(\"content-type\")",
        ] {
            let mut headers = http::HeaderMap::new();
            for bytes in &values {
                let mut value = http::HeaderValue::from_bytes(bytes).unwrap();
                value.set_sensitive(true);
                headers.append("content-type", value);
            }
            let mut session = MockPluginSession::new();
            session.expect_get_response_header().times(0);
            session.expect_response_header_map().times(1).return_const(headers);
            let output = if expression.contains("has_header") {
                expected.is_some().to_string()
            } else {
                expected.unwrap_or("missing").to_string()
            };
            session
                .expect_set_request_header()
                .times(1)
                .withf(move |name, value| name == "X-Result" && value == output)
                .returning(|_, _| Ok(()));
            let source = format!("let v = {expression}\nif v == nil {{ req.set_header(\"X-Result\", \"missing\") }} else {{ req.set_header(\"X-Result\", to_str(v)) }}");
            assert_eq!(run(&source, session), good_next());
        }
    }
}

#[test]
fn borrowed_response_presence_does_not_apply_string_limit() {
    let mut session = MockPluginSession::new();
    session.expect_get_response_header().times(0);
    session
        .expect_response_header_map()
        .times(1)
        .return_const(http::HeaderMap::from_iter([(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("long"),
        )]));
    session
        .expect_set_request_header()
        .times(1)
        .withf(|name, value| name == "X-Result" && value == "yes")
        .returning(|_, _| Ok(()));
    assert_eq!(
        run_with_limits(
            "if resp.has_header(\"content-type\") { req.set_header(\"X-Result\", \"yes\") }",
            session,
            VmLimits {
                max_string_len: 0,
                ..Default::default()
            },
        ),
        good_next()
    );
}

#[test]
fn test_resp_status_read() {
    let mut session = MockPluginSession::new();
    session.expect_get_response_status().returning(|| Some(404));
    session
        .expect_set_response_header()
        .withf(|h, v| h == "X-Status" && v == "not_found")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        "let s = resp.status\nif s != nil { if s == 404 { resp.set_header(\"X-Status\", \"not_found\") } }",
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_status_nil() {
    let mut session = MockPluginSession::new();
    session.expect_get_response_status().returning(|| None);
    session
        .expect_set_response_header()
        .withf(|h, v| h == "X-Status" && v == "no_status")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        "let s = resp.status\nif s == nil { resp.set_header(\"X-Status\", \"no_status\") }",
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_header_read() {
    let mut session = MockPluginSession::new();
    session.expect_get_response_header().times(0);
    session
        .expect_response_header_map()
        .return_const(http::HeaderMap::from_iter([(
            http::header::HeaderName::from_bytes(b"X-B").unwrap(),
            http::HeaderValue::from_static("hello"),
        )]));
    session
        .expect_set_response_header()
        .withf(|h, v| h == "X-Copy" && v == "hello")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        "let v = resp.header(\"X-B\")\nif v != nil { resp.set_header(\"X-Copy\", v) }",
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_header_nil() {
    let mut session = MockPluginSession::new();
    session.expect_get_response_header().times(0);
    session
        .expect_response_header_map()
        .return_const(http::HeaderMap::from_iter([]));
    session
        .expect_set_response_header()
        .withf(|h, v| h == "X-Copy" && v == "none")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        "let v = resp.header(\"X-B\")\nif v == nil { resp.set_header(\"X-Copy\", \"none\") }",
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_header_exact_byte_limit() {
    let mut session = MockPluginSession::new();
    session.expect_get_response_header().times(0);
    session
        .expect_response_header_map()
        .return_const(http::HeaderMap::from_iter([(
            http::header::HeaderName::from_bytes(b"X-B").unwrap(),
            http::HeaderValue::from_static("abcd"),
        )]));
    session
        .expect_set_response_header()
        .withf(|h, v| h == "X-Copy" && v == "abcd")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run_with_limits(
        r#"let v = resp.header("X-B")
if v != nil { resp.set_header("X-Copy", v) }"#,
        session,
        VmLimits {
            max_string_len: 4,
            ..Default::default()
        },
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_header_over_byte_limit() {
    let mut session = MockPluginSession::new();
    session.expect_get_response_header().times(0);
    session
        .expect_response_header_map()
        .return_const(http::HeaderMap::from_iter([(
            http::header::HeaderName::from_bytes(b"X-B").unwrap(),
            http::HeaderValue::from_static("abcd"),
        )]));
    session.expect_set_response_header().times(0);
    let err = run_expect_runtime_err_with_limits(
        r#"let v = resp.header("X-B")
if v != nil { resp.set_header("X-Copy", v) }"#,
        session,
        VmLimits {
            max_string_len: 3,
            ..Default::default()
        },
    );
    assert!(matches!(err, RuntimeError::StringTooLong { len: 4, limit: 3 }));
}

#[test]
fn test_resp_has_header_true() {
    let mut session = MockPluginSession::new();
    session.expect_get_response_header().times(0);
    session
        .expect_response_header_map()
        .return_const(http::HeaderMap::from_iter([(
            http::header::HeaderName::from_bytes(b"X-B").unwrap(),
            http::HeaderValue::from_static("val"),
        )]));
    session
        .expect_set_response_header()
        .withf(|h, v| h == "X-R" && v == "yes")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        "if resp.has_header(\"X-B\") { resp.set_header(\"X-R\", \"yes\") }",
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_has_header_false() {
    let mut session = MockPluginSession::new();
    session.expect_get_response_header().times(0);
    session
        .expect_response_header_map()
        .return_const(http::HeaderMap::from_iter([]));
    session
        .expect_set_response_header()
        .withf(|h, v| h == "X-R" && v == "no")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        "if resp.has_header(\"X-B\") == false { resp.set_header(\"X-R\", \"no\") }",
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_set_header() {
    let mut session = MockPluginSession::new();
    session
        .expect_set_response_header()
        .withf(|h, v| h == "X-C" && v == "val")
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_set_response_header()
        .withf(|h, v| h == "X-Result" && v == "nil")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        r#"let result = resp.set_header("X-C", "val")
if result == nil { resp.set_header("X-Result", "nil") }"#,
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_append_header() {
    let mut session = MockPluginSession::new();
    session
        .expect_append_response_header()
        .withf(|h, v| h == "Vary" && v == "Origin")
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_set_response_header()
        .withf(|h, v| h == "X-Result" && v == "nil")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        r#"let result = resp.append_header("Vary", "Origin")
if result == nil { resp.set_header("X-Result", "nil") }"#,
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_resp_remove_header() {
    let mut session = MockPluginSession::new();
    session
        .expect_remove_response_header()
        .withf(|h| h == "X-Remove")
        .times(1)
        .returning(|_| Ok(()));
    session
        .expect_set_response_header()
        .withf(|h, v| h == "X-Result" && v == "nil")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        r#"let result = resp.remove_header("X-Remove")
if result == nil { resp.set_header("X-Result", "nil") }"#,
        session,
    );
    assert_eq!(result, good_next());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_upstream_response_filter_phase() {
    use crate::plugins::http::dsl::config::DslConfig;
    use crate::plugins::http::dsl::plugin::DslPlugin;
    let mut session = MockPluginSession::new();
    session
        .expect_set_response_header()
        .withf(|h, v| h == "X-Phase" && v == "resp")
        .times(1)
        .returning(|_, _| Ok(()));

    let config = DslConfig {
        source: Some(r#"resp.set_header("X-Phase", "resp")"#.to_string()),
        name: "test-resp".into(),
        ..Default::default()
    };
    let plugin = DslPlugin::from_config_response(&config).expect("from_config_response failed");
    let mut log = PluginLog::new("test");
    let mut s = session;
    // run_upstream_response_filter uses block_in_place — requires multi-thread Tokio runtime
    let result = plugin.run_upstream_response_filter(&mut s as &mut dyn PluginSession, &mut log);
    assert_eq!(result, good_next());
}
