//! Tests for P3 utility functions: hmac_sha256, json_get, uuid, hex encode/decode, time_format, ip_in_cidr, join, log_warn, log_error.

use super::*;
use edgion_resources::sensitive::SENSITIVE_MODE;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

static MODE_LOCK: Mutex<()> = Mutex::new(());

struct ModeGuard<'a> {
    prev: u8,
    // Held purely for RAII: serializes tests that mutate SENSITIVE_MODE.
    _lock: std::sync::MutexGuard<'a, ()>,
}
impl Drop for ModeGuard<'_> {
    fn drop(&mut self) {
        SENSITIVE_MODE.store(self.prev, Ordering::SeqCst);
    }
}
fn set_mode(mode: u8) -> ModeGuard<'static> {
    let guard = MODE_LOCK.lock().unwrap();
    let prev = SENSITIVE_MODE.swap(mode, Ordering::SeqCst);
    ModeGuard { prev, _lock: guard }
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

// hmac_sha256 result must honor max_string_len like every other string builtin (F-005)
#[test]
fn test_hmac_sha256_result_too_long() {
    let session = MockPluginSession::new();
    let limits = VmLimits {
        max_string_len: 63,
        ..Default::default()
    };
    // HMAC-SHA-256 hex output is 64 chars > limit 63
    let result = run_with_limits(r#"let v = hmac_sha256("key", "data")"#, session, limits);
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
}

#[test]
fn test_json_get_string() {
    let source = r#"
        let val = json_get("{\"user\":{\"name\":\"alice\"}}", "$.user.name")
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
        let age = json_get("{\"age\": 30}", "$.age")
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
        let val = json_get("{\"a\": 1}", "$.b")
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
        let val = json_get("not json", "$.key")
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
        let val = json_get("[\"a\",\"b\",\"c\"]", "$[1]")
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

// time_format with an invalid chrono specifier returns ApiError (does not panic).
#[test]
fn test_time_format_invalid_specifier_returns_api_error() {
    let session = MockPluginSession::new();
    let err = run_expect_runtime_err(r#"time_format(0, "%Q")"#, session);
    assert!(
        matches!(
            &err,
            RuntimeError::ApiError { function, .. } if function == "time_format"
        ),
        "expected ApiError from time_format, got {:?}",
        err
    );
}

// ip_in_cidr with malformed CIDR returns false (not an error). Locks in the
// parts.len() != 2 / parse-fail fall-through paths in BuiltinId::IpInCidr.
#[test]
fn test_ip_in_cidr_invalid_cidr_returns_false() {
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Ok" && v == "false")
        .times(1)
        .returning(|_, _| Ok(()));
    let source = r#"
        let v = ip_in_cidr("1.2.3.4", "not-a-cidr")
        if v == false {
            req.set_header("X-Ok", "false")
        }
    "#;
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
    let vm = make_vm(source);
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    assert!(
        log.contains("something suspicious"),
        "log_warn value not recorded: {:?}",
        log.log
    );
}

#[test]
fn test_log_error() {
    let _g = set_mode(0);
    let source = r#"
        log_error("critical failure")
        return next()
    "#;
    let vm = make_vm(source);
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    assert!(
        log.contains("err: critical failure"),
        "log_error must emit unified `err: <value>` entry: {:?}",
        log.log
    );
}

#[test]
fn test_log_warn_replace_mode() {
    let _g = set_mode(1);
    let source = r#"
        log_warn("super-secret-value")
        return next()
    "#;
    let vm = make_vm(source);
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    assert!(log.contains("***"), "expected masked output: {:?}", log.log);
    assert!(
        !log.contains("super-secret-value"),
        "plaintext must not appear in replace mode"
    );
}

#[test]
fn test_log_warn_wrap_mode() {
    let _g = set_mode(2);
    let source = r#"
        log_warn("super-secret-value")
        return next()
    "#;
    let vm = make_vm(source);
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    assert!(
        log.contains("<E>super-secret-value</E>"),
        "expected wrapped output: {:?}",
        log.log
    );
}

#[test]
fn test_log_error_replace_mode() {
    let _g = set_mode(1);
    let source = r#"
        log_error("super-secret-value")
        return next()
    "#;
    let vm = make_vm(source);
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    assert!(
        log.contains("err: ***"),
        "expected `err:` prefix wrapping masked value: {:?}",
        log.log
    );
    assert!(
        !log.contains("super-secret-value"),
        "plaintext must not appear in replace mode"
    );
}

#[test]
fn test_log_error_wrap_mode() {
    let _g = set_mode(2);
    let source = r#"
        log_error("super-secret-value")
        return next()
    "#;
    let vm = make_vm(source);
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    assert!(
        log.contains("err: <E>super-secret-value</E>"),
        "expected `err:` prefix wrapping wrapped value: {:?}",
        log.log
    );
}

#[test]
fn test_log_fmt_repeated_large_shared_value_remains_bounded() {
    let source = format!(
        "let value = req.header(\"X-Large\")\nlet result = log_fmt(\"{}\", {})\nif result != nil {{ return deny(403, \"unexpected result\") }}\nlog_fmt(\"later entry\")\nreturn next()",
        "{}".repeat(254),
        vec!["value"; 254].join(", "),
    );
    let vm = make_vm_with_limits(
        &source,
        VmLimits {
            max_stack_depth: 256,
            max_string_len: 1024 * 1024,
            ..VmLimits::default()
        },
    );
    for mode in [0, 1, 2] {
        let _g = set_mode(mode);
        let mut session = MockPluginSession::new();
        session
            .expect_header_value()
            .withf(|name| name == "X-Large")
            .times(1)
            .returning(|_| Some("x".repeat(1024 * 1024)));
        let (result, log) = run_vm_policy_log(&vm, session, &DslErrorPolicy::Deny);
        assert_eq!(result, good_next());
        let json = serde_json::to_value(log).unwrap();
        assert_eq!(json["log"], serde_json::json!(["later entry", "OK"]));
        assert_eq!(json["log_full"], true);
    }
}

#[test]
fn test_log_fmt_preserves_scalar_coercion_and_body_error_after_oversized_prefix() {
    let _g = set_mode(0);
    let vm = make_vm(r#"log_fmt("{} {} {} {}", 42, true, false, nil)"#);
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    assert!(log.contains("42 true false nil"));
    for mode in [0, 1, 2] {
        SENSITIVE_MODE.store(mode, Ordering::SeqCst);
        let source = format!("log_fmt(\"{}{{}}\", req.body)", "x".repeat(513));
        let mut session = MockPluginSession::new();
        session.expect_declares_body().return_const(Some(true));
        session.expect_request_body_available_len().return_const(Ok(0));
        let error = run_expect_runtime_err(&source, session);
        assert!(matches!(
            error,
            RuntimeError::TypeError {
                expected: "a value implicitly convertible to Str",
                got: "Body",
                operation: "implicit string conversion",
            }
        ));
    }
}

#[test]
fn test_log_fmt_off_mode() {
    let _g = set_mode(0);
    let source = r#"
        log_fmt("secret: {}", "abc-token")
        return next()
    "#;
    let vm = make_vm(source);
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    assert!(
        log.contains("secret: abc-token"),
        "expected plain interpolation in off mode: {:?}",
        log.log
    );
}

#[test]
fn test_log_fmt_replace_mode_keeps_template() {
    let _g = set_mode(1);
    let source = r#"
        log_fmt("secret: {}", "abc-token")
        return next()
    "#;
    let vm = make_vm(source);
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    assert!(
        log.contains("secret: ***"),
        "expected template preserved with masked value: {:?}",
        log.log
    );
    assert!(
        !log.contains("abc-token"),
        "plaintext value must not appear in replace mode"
    );
}

#[test]
fn test_log_fmt_wrap_mode() {
    let _g = set_mode(2);
    let source = r#"
        log_fmt("secret: {}", "abc-token")
        return next()
    "#;
    let vm = make_vm(source);
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    assert!(
        log.contains("secret: <E>abc-token</E>"),
        "expected template plus wrapped value: {:?}",
        log.log
    );
}

#[test]
fn test_log_fmt_multiple_args_replace_mode() {
    let _g = set_mode(1);
    let source = r#"
        log_fmt("user {} ip {}", "alice", "1.2.3.4")
        return next()
    "#;
    let vm = make_vm(source);
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    assert!(
        log.contains("user *** ip ***"),
        "expected each arg masked in order: {:?}",
        log.log
    );
}

#[test]
fn test_log_fmt_multiple_args_off_mode_order() {
    let _g = set_mode(0);
    // Verifies positional ordering — args render left-to-right matching `{}` order.
    let source = r#"
        log_fmt("first={} second={}", "A", "B")
        return next()
    "#;
    let vm = make_vm(source);
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    assert!(
        log.contains("first=A second=B"),
        "args must render in source order: {:?}",
        log.log
    );
}

#[test]
fn test_log_fmt_zero_args() {
    // `log_fmt("no placeholders")` with zero values is permitted and degenerates
    // to a plain log entry.
    let _g = set_mode(0);
    let source = r#"
        log_fmt("a static event")
        return next()
    "#;
    let vm = make_vm(source);
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    assert!(log.contains("a static event"), "expected static output: {:?}", log.log);
}

#[test]
fn test_log_fmt_dynamic_template_rejected() {
    // Dynamic template would let request data flow into the unmasked plain part —
    // compile-time guard rejects non-literal templates.
    let source = r#"
        let t = "secret: {}"
        log_fmt(t, "abc")
        return next()
    "#;
    let result = compile_dsl_source(source, &ValidationLimits::default());
    assert!(result.is_err(), "dynamic template must be rejected at compile time");
    let err = format!("{:?}", result.unwrap_err());
    assert!(
        err.contains("template must be a string literal"),
        "expected literal-template diagnostic, got: {}",
        err
    );
}

#[test]
fn test_log_fmt_placeholder_arity_mismatch_rejected() {
    let result = compile_dsl_source(r#"log_fmt("a {} b {}", "only-one")"#, &ValidationLimits::default());
    assert!(result.is_err());
    let err = format!("{:?}", result.unwrap_err());
    assert!(
        err.contains("placeholder") && err.contains("value argument"),
        "expected placeholder/argc diagnostic, got: {}",
        err
    );
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

// time_format(0, "") — empty format string; chrono DelayedFormat::fmt returns Ok("") → Value::Str("").
#[test]
fn test_time_format_empty_format_returns_empty() {
    assert_sets_request_header(r#"req.set_header("X-Out", time_format(0, ""))"#, "X-Out", "");
}

// time_format(0, "%") — trailing % with no specifier; chrono returns Err → ApiError{function:"time_format"}.
#[test]
fn test_time_format_trailing_percent_returns_api_error() {
    let session = MockPluginSession::new();
    let err = run_expect_runtime_err(r#"time_format(0, "%")"#, session);
    assert!(
        matches!(
            &err,
            RuntimeError::ApiError { function, .. } if function == "time_format"
        ),
        "expected ApiError from time_format, got {:?}",
        err
    );
}

// time_format(i64::MAX, "%Y") — timestamp out of chrono's representable range → Value::Nil.
#[test]
fn test_time_format_out_of_range_timestamp_returns_nil() {
    assert_sets_request_header(
        r#"let v = time_format(9223372036854775807, "%Y")
if v == nil { req.set_header("X-Out", "nil") }"#,
        "X-Out",
        "nil",
    );
}
