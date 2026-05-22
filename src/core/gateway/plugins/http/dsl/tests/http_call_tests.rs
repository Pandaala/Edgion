//! Tests for map literal syntax, Index access, and http.call guards.
//! http.call runtime behavior with real responses requires a live HTTP server — integration tests cover that.

use super::*;

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
