//! Tests for P3 utility functions: hmac_sha256, json_get, uuid, hex encode/decode, time_format, ip_in_cidr, join, log_warn, log_error.

use super::*;
use crate::types::sensitive::SENSITIVE_MODE;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

static MODE_LOCK: Mutex<()> = Mutex::new(());

struct ModeGuard<'a>(u8, std::sync::MutexGuard<'a, ()>);
impl Drop for ModeGuard<'_> {
    fn drop(&mut self) {
        SENSITIVE_MODE.store(self.0, Ordering::SeqCst);
    }
}
fn set_mode(mode: u8) -> ModeGuard<'static> {
    let guard = MODE_LOCK.lock().unwrap();
    let prev = SENSITIVE_MODE.swap(mode, Ordering::SeqCst);
    ModeGuard(prev, guard)
}

#[test]
fn test_hmac_sha256() {
    let source = r#"
        let sig = hmac_sha256("secret-key", "hello")
        req.set_header("X-Sig", sig)
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|n, v| {
            n == "X-Sig" && v.len() == 64 // SHA256 hex = 64 chars
        })
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_json_get_string() {
    let source = r#"
        let val = json_get("{\"user\":{\"name\":\"alice\"}}", "user.name")
        req.set_header("X-Name", val)
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Name" && v == "alice")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_json_get_number() {
    let source = r#"
        let age = json_get("{\"age\": 30}", "age")
        if age == 30 {
            req.set_header("X-Age", "thirty")
        }
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Age" && v == "thirty")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_json_get_missing_key() {
    let source = r#"
        let val = json_get("{\"a\": 1}", "b")
        if val == nil {
            req.set_header("X-Found", "no")
        }
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Found" && v == "no")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_json_get_invalid_json() {
    let source = r#"
        let val = json_get("not json", "key")
        if val == nil {
            req.set_header("X-Valid", "no")
        }
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Valid" && v == "no")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_json_get_array_index() {
    let source = r#"
        let val = json_get("[\"a\",\"b\",\"c\"]", "1")
        req.set_header("X-Val", val)
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Val" && v == "b")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_uuid_format() {
    let source = r#"
        let id = uuid()
        if len(id) == 36 {
            req.set_header("X-Has-UUID", "yes")
        }
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Has-UUID" && v == "yes")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_hex_encode() {
    let source = r#"req.set_header("X-Hex", hex_encode("AB"))"#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Hex" && v == "4142")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_hex_decode() {
    let source = r#"req.set_header("X-Dec", hex_decode("4142"))"#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Dec" && v == "AB")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_hex_decode_invalid() {
    let source = r#"
        let val = hex_decode("not_hex!")
        if val == nil {
            req.set_header("X-Ok", "nil")
        }
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Ok" && v == "nil")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_time_format() {
    let source = r#"
        let ts = 1609459200
        let formatted = time_format(ts, "%Y-%m-%d")
        req.set_header("X-Date", formatted)
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Date" && v == "2021-01-01")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_ip_in_cidr_v4_match() {
    let source = r#"
        if ip_in_cidr("10.0.1.5", "10.0.0.0/8") {
            req.set_header("X-Net", "internal")
        }
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Net" && v == "internal")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_ip_in_cidr_v4_no_match() {
    let source = r#"
        if ip_in_cidr("192.168.1.1", "10.0.0.0/8") {
            req.set_header("X-Net", "internal")
        } else {
            req.set_header("X-Net", "external")
        }
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Net" && v == "external")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_ip_in_cidr_v6() {
    let source = r#"
        if ip_in_cidr("::1", "::1/128") {
            req.set_header("X-Net", "loopback")
        }
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Net" && v == "loopback")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_ip_in_cidr_invalid_ip() {
    let source = r#"
        if ip_in_cidr("not-an-ip", "10.0.0.0/8") {
            req.set_header("X-Match", "yes")
        } else {
            req.set_header("X-Match", "no")
        }
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Match" && v == "no")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_join() {
    let source = r#"
        let parts = split("a,b,c", ",")
        let joined = join(parts, "-")
        req.set_header("X-Joined", joined)
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Joined" && v == "a-b-c")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_log_warn() {
    let _g = set_mode(0);
    let source = r#"
        log_warn("something suspicious")
        return next()
    "#;
    let session = MockPluginSession::new();
    let vm = make_vm(source);
    let mut log = PluginLog::new("test");
    let mut s = session;
    let result = execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    assert!(log.contains("something suspicious"), "log_warn value not recorded: {:?}", log.log);
}

#[test]
fn test_log_error() {
    let _g = set_mode(0);
    let source = r#"
        log_error("critical failure")
        return next()
    "#;
    let session = MockPluginSession::new();
    let vm = make_vm(source);
    let mut log = PluginLog::new("test");
    let mut s = session;
    let result = execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    assert!(log.contains("critical failure"), "log_error value not recorded: {:?}", log.log);
}

#[test]
fn test_log_warn_replace_mode() {
    let _g = set_mode(1);
    let source = r#"
        log_warn("super-secret-value")
        return next()
    "#;
    let session = MockPluginSession::new();
    let vm = make_vm(source);
    let mut log = PluginLog::new("test");
    let mut s = session;
    let result = execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    assert!(log.contains("***"), "expected masked output: {:?}", log.log);
    assert!(!log.contains("super-secret-value"), "plaintext must not appear in replace mode");
}

#[test]
fn test_log_warn_wrap_mode() {
    let _g = set_mode(2);
    let source = r#"
        log_warn("super-secret-value")
        return next()
    "#;
    let session = MockPluginSession::new();
    let vm = make_vm(source);
    let mut log = PluginLog::new("test");
    let mut s = session;
    let result = execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    assert!(log.contains("<E>super-secret-value</E>"), "expected wrapped output: {:?}", log.log);
}

#[test]
fn test_log_error_replace_mode() {
    let _g = set_mode(1);
    let source = r#"
        log_error("super-secret-value")
        return next()
    "#;
    let session = MockPluginSession::new();
    let vm = make_vm(source);
    let mut log = PluginLog::new("test");
    let mut s = session;
    let result = execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    assert!(log.contains("***"), "expected masked output: {:?}", log.log);
    assert!(!log.contains("super-secret-value"), "plaintext must not appear in replace mode");
}

#[test]
fn test_log_error_wrap_mode() {
    let _g = set_mode(2);
    let source = r#"
        log_error("super-secret-value")
        return next()
    "#;
    let session = MockPluginSession::new();
    let vm = make_vm(source);
    let mut log = PluginLog::new("test");
    let mut s = session;
    let result = execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    assert!(log.contains("<E>super-secret-value</E>"), "expected wrapped output: {:?}", log.log);
}

#[test]
fn test_hmac_sha256_verify() {
    // Verify a known HMAC value
    let source = r#"
        let sig = hmac_sha256("key", "data")
        req.set_header("X-Sig", sig)
    "#;
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|n, v| {
            // Known HMAC-SHA256("key", "data") value
            n == "X-Sig" && v == "5031fe3d989c6d1537a013fa6e739da23463fdaec3b70137d828e36ace221bd0"
        })
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}
