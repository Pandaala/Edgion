//! Tests for map literal syntax, Index access, and http.call guards.
//! http.call runtime behavior with real responses requires a live HTTP server — integration tests cover that.

use super::*;
use async_trait::async_trait;
use bytes::Bytes;
use std::collections::VecDeque;

use crate::plugins::runtime::traits::session::{BodyReadError, RequestBodySnapshotReader};

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
    // Verify http.call compiles; no actual network call is made
    // (network tests belong in integration tests)
    let source = r#"
        let http_resp = http.call("GET", "http://127.0.0.1:1", nil)
        return next()
    "#;
    let result = run(source, MockPluginSession::new());
    // Should succeed (http.call returns status=0 on connection failure, never panics)
    assert_eq!(result, good_next());
}

#[test]
fn test_http_call_failure_returns_status_zero() {
    // Connecting to a closed port returns status=0, no exception
    let source = r#"
        let http_resp = http.call("GET", "http://127.0.0.1:1", nil)
        if http_resp["status"] == 0 {
            return next()
        }
        return deny(500, "expected status 0 on failure")
    "#;
    let result = run(source, MockPluginSession::new());
    assert_eq!(result, good_next());
}

#[test]
fn test_http_call_with_timeout() {
    let source = r#"
        let http_resp = http.call("GET", "http://127.0.0.1:1", nil, 100)
        if http_resp["status"] == 0 {
            return next()
        }
        return deny(500, "expected status 0")
    "#;
    let result = run(source, MockPluginSession::new());
    assert_eq!(result, good_next());
}

#[test]
fn test_http_call_with_headers_map() {
    let source = r#"
        let headers = {"Authorization": "Bearer token123"}
        let http_resp = http.call("GET", "http://127.0.0.1:1", nil, 100, headers)
        if http_resp["status"] == 0 {
            return next()
        }
        return deny(500, "expected status 0")
    "#;
    let result = run(source, MockPluginSession::new());
    assert_eq!(result, good_next());
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
fn test_http_call_caps_large_response_body() {
    // A response body larger than max_response_body_bytes must be streamed and truncated to
    // the cap, never fully buffered first. Unlike the other tests in this file, this one
    // needs a live response to exercise the body-read path, so it drives a loopback server
    // in-process (deterministic, no external dependency). The server returns 100 bytes;
    // with the cap set to 4 the script must observe exactly the first 4 bytes.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut throwaway = [0u8; 1024];
            let _ = sock.read(&mut throwaway).await;
            let body = vec![b'A'; 100];
            let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
            let _ = sock.write_all(header.as_bytes()).await;
            let _ = sock.write_all(&body).await;
            let _ = sock.shutdown().await;
        });
        let url = format!("http://{}/", addr);
        let source = r#"
            let r = http.call("GET", "__URL__", nil)
            if r["status"] == 200 {
                if r["body"] == "AAAA" {
                    return next()
                }
            }
            return deny(500, "body not capped")
        "#
        .replace("__URL__", &url);

        let vm = make_vm(&source);
        let mut log = PluginLog::new("test");
        let mut session = MockPluginSession::new();
        let runtime = DslRuntime {
            max_response_body_bytes: 4,
            ..DslRuntime::default()
        };
        let result = execute_safe(
            &vm,
            &mut session as &mut dyn PluginSession,
            &mut log,
            &DslErrorPolicy::Deny,
            &runtime,
        )
        .await;
        assert_eq!(result, good_next());
    });
}

#[test]
fn test_http_call_body_truncated_to_max_string_len() {
    // After streaming-truncation at max_response_body_bytes, r["body"] must also honor
    // the sandbox-wide max_string_len. Without that cap, a body up to
    // max_response_body_bytes (default 1 MB = 128x max_string_len) would flow into
    // req.set_header / ctx.set / log without any further size gate. The truncation is
    // silent (not an error) to preserve http.call's documented "never raises a runtime
    // error" contract — mirroring the existing silent caps on body bytes
    // (max_response_body_bytes) and headers map size (max_map_size).
    // Setup: server returns 16 bytes, default max_response_body_bytes (1 MB) so the
    // body-bytes cap does not fire, but max_string_len = 4 forces the new truncation.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut throwaway = [0u8; 1024];
            let _ = sock.read(&mut throwaway).await;
            let body = vec![b'A'; 16];
            let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
            let _ = sock.write_all(header.as_bytes()).await;
            let _ = sock.write_all(&body).await;
            let _ = sock.shutdown().await;
        });
        let url = format!("http://{}/", addr);
        let source = r#"
            let r = http.call("GET", "__URL__", nil)
            if r["status"] == 200 {
                if r["body"] == "AAAA" {
                    return next()
                }
            }
            return deny(500, "body not truncated to max_string_len")
        "#
        .replace("__URL__", &url);

        let limits = VmLimits {
            max_string_len: 4,
            ..VmLimits::default()
        };
        let vm = make_vm_with_limits(&source, limits);
        let mut log = PluginLog::new("test");
        let mut session = MockPluginSession::new();
        let runtime = DslRuntime::default();
        let result = execute_safe(
            &vm,
            &mut session as &mut dyn PluginSession,
            &mut log,
            &DslErrorPolicy::Deny,
            &runtime,
        )
        .await;
        assert_eq!(result, good_next());
    });
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
