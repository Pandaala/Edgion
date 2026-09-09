//! Tests for map literal syntax, Index access, and `http.call` guards.
//! Response completion uses synthetic response streams so unit tests can cover
//! hard bounds without network access; end-to-end integration coverage remains frozen.

use super::*;
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::plugins::http::dsl::lang::builtins::complete_http_call_response;
use crate::plugins::http::dsl::lang::value::Value;
use crate::plugins::http::dsl::runtime::dsl_http_client;
use crate::plugins::runtime::traits::session::{BodyReadError, RequestBodySnapshotReader};

fn finish_response(
    send_result: Result<reqwest::Response, reqwest::Error>,
    max_response_body_bytes: usize,
    max_string_len: usize,
    max_map_size: usize,
) -> Value {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(complete_http_call_response(
            send_result,
            max_response_body_bytes,
            max_string_len,
            max_map_size,
        ))
}

fn streamed_response(parts: Vec<Result<Bytes, io::Error>>) -> reqwest::Response {
    let body = reqwest::Body::wrap_stream(futures::stream::iter(parts));
    http::Response::new(body).into()
}

fn result_map(value: Value) -> Box<indexmap::IndexMap<String, Value>> {
    match value {
        Value::Map(result) => result,
        other => panic!("expected Map, got {other:?}"),
    }
}

fn assert_http_failure(value: Value) {
    let result = result_map(value);
    assert_eq!(result.get("status"), Some(&Value::Int(0)));
    assert_eq!(result.get("body"), Some(&Value::Str("".into())));
    assert_eq!(result.get("headers"), Some(&Value::Map(Box::default())));
}

struct StaticSnapshotReader {
    len: u64,
    chunks: VecDeque<Bytes>,
}

impl StaticSnapshotReader {
    fn new(chunks: impl IntoIterator<Item = Bytes>) -> Self {
        let chunks: VecDeque<_> = chunks.into_iter().collect();
        let len = chunks.iter().map(|chunk| chunk.len() as u64).sum();
        Self { len, chunks }
    }
}

#[async_trait]
impl RequestBodySnapshotReader for StaticSnapshotReader {
    fn len(&self) -> u64 {
        self.len
    }

    async fn next_chunk(&mut self) -> Result<Option<Bytes>, BodyReadError> {
        Ok(self.chunks.pop_front())
    }
}

fn opaque_body_session(reader: StaticSnapshotReader, physical_cap: u64) -> MockPluginSession {
    let mut session = MockPluginSession::new();
    session.expect_declares_body().times(2).return_const(Some(true));
    session
        .expect_request_body_available_len()
        .withf(|known_bodyless| !known_bodyless)
        .return_const(Ok(reader.len()));
    session
        .expect_request_body_snapshot_reader()
        .withf(|known_bodyless| !known_bodyless)
        .times(1)
        .return_once(move |_| Ok(Box::new(reader)));
    let mut ctx = crate::EdgionHttpContext::new();
    ctx.request_body_config.max_body_size_bytes = physical_cap;
    session.expect_ctx().times(0..=1).return_const(ctx);
    session
}

fn body_token_session(len: u64) -> MockPluginSession {
    let mut session = MockPluginSession::new();
    session.expect_declares_body().times(1).return_const(Some(true));
    session
        .expect_request_body_available_len()
        .withf(|known_bodyless| !known_bodyless)
        .times(1)
        .return_const(Ok(len));
    session
}

#[test]
fn test_map_literal_compiles() {
    let source = r#"
        let m = {"key": "value", "num": "42"}
        return next()
    "#;
    let result = run(source, MockPluginSession::new());
    assert_eq!(result, good_next());
}

#[test]
fn test_map_literal_empty_compiles() {
    let source = r#"
        let m = {}
        return next()
    "#;
    let result = run(source, MockPluginSession::new());
    assert_eq!(result, good_next());
}

#[test]
fn test_index_access_on_map() {
    // Map index access with string key
    let source = r#"
        let m = {"status": "200", "body": "ok"}
        let s = m["status"]
        return next()
    "#;
    let result = run(source, MockPluginSession::new());
    assert_eq!(result, good_next());
}

#[test]
fn test_index_access_missing_key_returns_nil() {
    let source = r#"
        let m = {"a": "1"}
        let v = m["missing"]
        if v == nil {
            return next()
        }
        return deny(400, "expected nil")
    "#;
    let result = run(source, MockPluginSession::new());
    assert_eq!(result, good_next());
}

#[test]
fn test_map_in_condition() {
    let source = r#"
        let m = {"flag": "yes"}
        if m["flag"] == "yes" {
            return next()
        }
        return deny(400, "condition failed")
    "#;
    let result = run(source, MockPluginSession::new());
    assert_eq!(result, good_next());
}

#[test]
fn test_http_call_compile_only() {
    let source = r#"
        let http_resp = http.call("GET", "http://127.0.0.1:1", nil)
        return next()
    "#;
    let _vm = make_vm(source);
}

#[test]
fn test_http_call_send_error_returns_status_zero_without_network() {
    let send_result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(dsl_http_client(false, true).get("http://[").send());
    assert!(send_result.as_ref().unwrap_err().is_builder());
    assert_http_failure(finish_response(send_result, 1024, 1024, 16));
}

#[test]
fn test_http_call_with_timeout_compiles() {
    let source = r#"
        let http_resp = http.call("GET", "http://127.0.0.1:1", nil, 100)
        return next()
    "#;
    let _vm = make_vm(source);
}

#[test]
fn test_http_call_with_headers_map_compiles() {
    let source = r#"
        let headers = {"Authorization": "Bearer token123"}
        let http_resp = http.call("GET", "http://127.0.0.1:1", nil, 100, headers)
        return next()
    "#;
    let _vm = make_vm(source);
}

#[test]
fn test_http_call_rejects_body_framing_headers_before_dial() {
    let source = r#"
        let headers = {"Transfer-Encoding": "chunked"}
        let http_resp = http.call("POST", "http://127.0.0.1:1", req.body, 100, headers)
        return next()
    "#;
    let session = opaque_body_session(StaticSnapshotReader::new([Bytes::from_static(b"payload")]), 1024);
    let error = run_expect_runtime_err(source, session);
    assert!(matches!(
        error,
        RuntimeError::ApiError {
            ref function,
            ref message,
        } if function == "http.call"
            && message == "Content-Length and Transfer-Encoding are managed by http.call for Body payloads"
    ));
}

#[test]
fn test_http_call_rejects_body_above_request_cap_before_dial() {
    let source = r#"
        let http_resp = http.call("POST", "http://127.0.0.1:1", req.body)
        return next()
    "#;
    let session = opaque_body_session(StaticSnapshotReader::new([Bytes::from_static(b"12345")]), 1024);
    let runtime = DslRuntime {
        max_request_body_bytes: 4,
        ..DslRuntime::default()
    };
    let vm = make_vm(source);
    let mut log = PluginLog::new("test");
    let mut session = session;
    let error = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(vm.execute(&mut session as &mut dyn PluginSession, &mut log, &runtime))
        .expect_err("request Body above the http.call cap must fail before dialing");
    assert!(matches!(
        error,
        RuntimeError::BodyAccess { ref message }
            if message == "request Body exceeds http.call limit of 4 bytes"
    ));
}

#[test]
fn test_http_call_body_unavailable_is_hard_body_access_error() {
    let source = r#"
        let http_resp = http.call("POST", "http://127.0.0.1:1", req.body)
        return next()
    "#;
    let mut session = MockPluginSession::new();
    session.expect_declares_body().times(2).return_const(Some(true));
    session.expect_request_body_available_len().return_const(Ok(7));
    session
        .expect_request_body_snapshot_reader()
        .return_once(|_| Err(BodyReadError::StorageFailure));
    let error = run_expect_runtime_err(source, session);
    assert!(matches!(
        error,
        RuntimeError::BodyAccess { ref message } if message == "body capture storage failed"
    ));
}

#[test]
fn test_http_call_rejects_body_in_method_position() {
    let error = run_expect_runtime_err(
        r#"let result = http.call(req.body, "http://127.0.0.1:1", nil)"#,
        body_token_session(7),
    );
    assert!(matches!(error, RuntimeError::TypeError { got: "Body", .. }));
}

#[test]
fn test_http_call_rejects_body_in_url_position() {
    let error = run_expect_runtime_err(
        r#"let result = http.call("POST", req.body, nil)"#,
        body_token_session(7),
    );
    assert!(matches!(error, RuntimeError::TypeError { got: "Body", .. }));
}

#[test]
fn test_http_call_rejects_body_in_timeout_position() {
    let error = run_expect_runtime_err(
        r#"let result = http.call("POST", "http://127.0.0.1:1", nil, req.body)"#,
        body_token_session(7),
    );
    assert!(matches!(
        error,
        RuntimeError::TypeError {
            got: "Body",
            operation: "http.call timeout_ms",
            ..
        }
    ));
}

#[test]
fn test_http_call_rejects_body_in_header_value_position() {
    let error = run_expect_runtime_err(
        r#"let result = http.call("POST", "http://127.0.0.1:1", nil, 100, {"X-Test": req.body})"#,
        body_token_session(7),
    );
    assert!(matches!(
        error,
        RuntimeError::TypeError {
            got: "Body",
            operation: "http.call headers_map",
            ..
        }
    ));
}

#[test]
fn test_http_call_rejects_non_http_scheme() {
    // file:// scheme must be rejected with an error (not silently fail)
    let source = r#"
        let http_resp = http.call("GET", "file:///etc/passwd", nil)
        return next()
    "#;
    let result = run(source, MockPluginSession::new());
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
}

#[test]
fn test_http_call_block_private_ips_rejects_loopback() {
    let source = r#"
        let http_resp = http.call("GET", "http://127.0.0.1:1", nil)
        return next()
    "#;
    let runtime = DslRuntime {
        block_private_ips: true,
        ..DslRuntime::default()
    };
    let result = run_with_runtime(source, MockPluginSession::new(), runtime);
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
}

#[test]
fn test_http_call_block_private_ips_rejects_ipv4_mapped_ipv6() {
    let source = r#"
        let http_resp = http.call("GET", "http://[::ffff:192.168.1.1]:80", nil)
        return next()
    "#;
    let runtime = DslRuntime {
        block_private_ips: true,
        ..DslRuntime::default()
    };
    let result = run_with_runtime(source, MockPluginSession::new(), runtime);
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
}

#[test]
fn test_http_call_complete_response_preserves_status_body_and_lowercase_header() {
    let response = http::Response::builder()
        .status(201)
        .header("X-Mixed-Case", "value")
        .body(reqwest::Body::from("ok"))
        .unwrap()
        .into();
    let result = result_map(finish_response(Ok(response), 16, 16, 4));
    assert_eq!(result.get("status"), Some(&Value::Int(201)));
    assert_eq!(result.get("body"), Some(&Value::Str("ok".into())));
    let Some(Value::Map(headers)) = result.get("headers") else {
        panic!("expected response headers Map");
    };
    assert_eq!(headers.get("x-mixed-case"), Some(&Value::Str("value".into())));
}

#[test]
fn test_http_call_declared_oversize_fails_without_polling_body() {
    let body = reqwest::Body::wrap_stream(futures::stream::poll_fn(
        |_| -> std::task::Poll<Option<Result<Bytes, io::Error>>> {
            panic!("declared oversize body must not be polled")
        },
    ));
    let response = http::Response::builder()
        .header(reqwest::header::CONTENT_LENGTH, "5")
        .body(body)
        .unwrap()
        .into();
    assert_http_failure(finish_response(Ok(response), 4, 8, 4));
}

#[test]
fn test_http_call_streamed_oversize_fails_without_polling_tail() {
    let parts = futures::stream::iter([
        Ok::<_, io::Error>(Bytes::from_static(b"abcd")),
        Ok(Bytes::from_static(b"e")),
    ])
    .chain(futures::stream::poll_fn(
        |_| -> std::task::Poll<Option<Result<Bytes, io::Error>>> { panic!("oversize body tail must not be polled") },
    ));
    let response = http::Response::new(reqwest::Body::wrap_stream(parts)).into();
    assert_http_failure(finish_response(Ok(response), 4, 8, 4));
}

#[test]
fn test_http_call_max_string_len_is_an_effective_body_cap() {
    let response = streamed_response(vec![Ok(Bytes::from_static(b"abcde"))]);
    assert_http_failure(finish_response(Ok(response), 16, 4, 4));
}

#[test]
fn test_http_call_exact_cap_succeeds_only_after_eof() {
    let eof_seen = Arc::new(AtomicBool::new(false));
    let eof_seen_by_stream = Arc::clone(&eof_seen);
    let mut first = true;
    let stream = futures::stream::poll_fn(move |_| {
        if first {
            first = false;
            std::task::Poll::Ready(Some(Ok::<_, io::Error>(Bytes::from_static(b"abcd"))))
        } else {
            eof_seen_by_stream.store(true, Ordering::Release);
            std::task::Poll::Ready(None)
        }
    });
    let response = http::Response::new(reqwest::Body::wrap_stream(stream)).into();
    let result = result_map(finish_response(Ok(response), 4, 8, 4));
    assert_eq!(result.get("status"), Some(&Value::Int(200)));
    assert_eq!(result.get("body"), Some(&Value::Str("abcd".into())));
    assert!(eof_seen.load(Ordering::Acquire));
}

#[test]
fn test_http_call_exact_cap_followed_by_another_byte_fails() {
    let response = streamed_response(vec![Ok(Bytes::from_static(b"abcd")), Ok(Bytes::from_static(b"e"))]);
    assert_http_failure(finish_response(Ok(response), 4, 8, 4));
}

#[test]
fn test_http_call_zero_cap_accepts_empty_body_after_eof() {
    let eof_seen = Arc::new(AtomicBool::new(false));
    let eof_seen_by_stream = Arc::clone(&eof_seen);
    let stream = futures::stream::poll_fn(move |_| {
        eof_seen_by_stream.store(true, Ordering::Release);
        std::task::Poll::Ready(None::<Result<Bytes, io::Error>>)
    });
    let response = http::Response::new(reqwest::Body::wrap_stream(stream)).into();
    let result = result_map(finish_response(Ok(response), 0, 8, 4));
    assert_eq!(result.get("status"), Some(&Value::Int(200)));
    assert_eq!(result.get("body"), Some(&Value::Str("".into())));
    assert!(eof_seen.load(Ordering::Acquire));
}

#[test]
fn test_http_call_zero_cap_rejects_first_nonempty_byte_without_polling_tail() {
    let parts = futures::stream::once(async { Ok::<_, io::Error>(Bytes::from_static(b"x")) }).chain(
        futures::stream::poll_fn(|_| -> std::task::Poll<Option<Result<Bytes, io::Error>>> {
            panic!("zero-cap response tail must not be polled")
        }),
    );
    let response = http::Response::new(reqwest::Body::wrap_stream(parts)).into();
    assert_http_failure(finish_response(Ok(response), 0, 8, 4));
}

#[test]
fn test_http_call_lossy_utf8_expansion_is_bounded_before_conversion() {
    let response = streamed_response(vec![Ok(Bytes::from_static(&[0xFF]))]);
    let result = result_map(finish_response(Ok(response), 3, 3, 4));
    let Some(Value::Str(body)) = result.get("body") else {
        panic!("expected response body Str");
    };
    assert_eq!(body.len(), 3);
    assert_eq!(body.chars().count(), 1);
    assert!(!body.is_ascii());

    let response = streamed_response(vec![Ok(Bytes::from_static(&[0xFF]))]);
    assert_http_failure(finish_response(Ok(response), 2, 2, 4));
}

#[test]
fn test_http_call_retained_oversized_header_name_fails() {
    let response = http::Response::builder()
        .header("xlong", "ok")
        .body(reqwest::Body::from(""))
        .unwrap()
        .into();
    assert_http_failure(finish_response(Ok(response), 8, 4, 4));
}

#[test]
fn test_http_call_retained_oversized_header_value_fails() {
    let response = http::Response::builder()
        .header("x", "long-value")
        .body(reqwest::Body::from(""))
        .unwrap()
        .into();
    assert_http_failure(finish_response(Ok(response), 8, 4, 4));
}

#[test]
fn test_http_call_non_utf8_header_is_skipped_before_name_limit_and_slot_count() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "oversized-name",
        reqwest::header::HeaderValue::from_bytes(&[0xFF]).unwrap(),
    );
    headers.insert("a", reqwest::header::HeaderValue::from_static("one"));
    headers.insert("b", reqwest::header::HeaderValue::from_static("two"));
    let mut response = http::Response::new(reqwest::Body::from(""));
    *response.headers_mut() = headers;
    let result = result_map(finish_response(Ok(response.into()), 8, 3, 3));
    let Some(Value::Map(headers)) = result.get("headers") else {
        panic!("expected response headers Map");
    };
    assert_eq!(headers.len(), 2);
    assert_eq!(headers.get("a"), Some(&Value::Str("one".into())));
    assert_eq!(headers.get("b"), Some(&Value::Str("two".into())));
}

#[test]
fn test_http_call_duplicate_header_replaces_value_while_prefix_is_open() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.append("x", reqwest::header::HeaderValue::from_static("old"));
    headers.append("x", reqwest::header::HeaderValue::from_static("new"));
    let mut response = http::Response::new(reqwest::Body::from(""));
    *response.headers_mut() = headers;
    let result = result_map(finish_response(Ok(response.into()), 8, 3, 2));
    let Some(Value::Map(headers)) = result.get("headers") else {
        panic!("expected response headers Map");
    };
    assert_eq!(headers.get("x"), Some(&Value::Str("new".into())));
}

#[test]
fn test_http_call_duplicate_header_is_ignored_after_map_is_full() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.append("x", reqwest::header::HeaderValue::from_static("old"));
    headers.append("x", reqwest::header::HeaderValue::from_static("new"));
    let mut response = http::Response::new(reqwest::Body::from(""));
    *response.headers_mut() = headers;
    let result = result_map(finish_response(Ok(response.into()), 8, 3, 1));
    let Some(Value::Map(headers)) = result.get("headers") else {
        panic!("expected response headers Map");
    };
    assert_eq!(headers.get("x"), Some(&Value::Str("old".into())));
}

#[test]
fn test_http_call_ignores_oversized_tail_after_header_map_is_full() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("a", reqwest::header::HeaderValue::from_static("ok"));
    headers.insert("b", reqwest::header::HeaderValue::from_static("ok"));
    let tail = headers.keys().last().expect("two headers have a tail").clone();
    *headers.get_mut(&tail).expect("tail header is present") = reqwest::header::HeaderValue::from_static("too-long");
    let mut response = http::Response::new(reqwest::Body::from(""));
    *response.headers_mut() = headers;
    let result = result_map(finish_response(Ok(response.into()), 8, 3, 1));
    assert_eq!(result.get("status"), Some(&Value::Int(200)));
    let Some(Value::Map(headers)) = result.get("headers") else {
        panic!("expected response headers Map");
    };
    assert_eq!(headers.len(), 1);
    assert!(!headers.contains_key(tail.as_str()));
}

#[test]
fn test_http_call_stream_error_discards_valid_prefix() {
    let response = streamed_response(vec![
        Ok(Bytes::from_static(b"ok")),
        Err(io::Error::other("private upstream detail")),
    ]);
    assert_http_failure(finish_response(Ok(response), 8, 8, 4));
}

#[test]
fn test_map_len() {
    let source = r#"
        let m = {"a": "1", "b": "2", "c": "3"}
        if len(m) == 3 {
            return next()
        }
        return deny(500, "wrong len")
    "#;
    let result = run(source, MockPluginSession::new());
    assert_eq!(result, good_next());
}
