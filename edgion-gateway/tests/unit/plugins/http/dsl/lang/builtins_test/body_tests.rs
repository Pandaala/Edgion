use super::super::validator::{compile_dsl_source, ValidationLimits};
use super::super::vm::{execute_safe, DslErrorPolicy, Vm, VmLimits};
use super::{body_bytes_to_value, checked_bounded_len, decode_http_call_body, lossy_utf8_output_len};
use crate::plugins::http::dsl::lang::bytecode::CompiledScript;
use crate::plugins::http::dsl::runtime::DslRuntime;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::{PluginLog, PluginSession};
use crate::request_body::BodyReadError;
use bytes::Bytes;
use edgion_resources::filters::PluginRunningResult;

use super::RuntimeError;
use super::Value;

#[test]
fn checked_bounded_len_rejects_limit_and_integer_overflow() {
    assert_eq!(checked_bounded_len(3, 2, 5), Some(5));
    assert_eq!(checked_bounded_len(3, 3, 5), None);
    assert_eq!(checked_bounded_len(usize::MAX, 1, usize::MAX), None);
}

#[test]
fn lossy_utf8_output_len_preflights_replacement_expansion() {
    assert_eq!(lossy_utf8_output_len(b"plain", 5), Some(5));
    assert_eq!(lossy_utf8_output_len(&[0xFF], 3), Some(3));
    assert_eq!(lossy_utf8_output_len(&[0xFF], 2), None);
    assert_eq!(lossy_utf8_output_len(&[b'a', 0xFF, b'b'], 5), Some(5));
    assert_eq!(lossy_utf8_output_len(&[0xF0, 0x90, 0x80], 3), Some(3));
}

#[test]
fn http_body_decode_preserves_lossy_bytes_and_prefix_limits() {
    let mut cases = vec![
        Bytes::new(),
        Bytes::from_static(b"plain"),
        Bytes::from_static("é€".as_bytes()),
    ];
    for prefix in [String::new(), "a".into(), "é€".into(), "a".repeat(4096)] {
        for invalid in [&[0xFF][..], &[0x80][..], &[0xFF, 0x80][..], &[0xE2, 0x82][..]] {
            for suffix in ["", "tail", "€"] {
                let mut bytes = prefix.as_bytes().to_vec();
                bytes.extend_from_slice(invalid);
                bytes.extend_from_slice(suffix.as_bytes());
                cases.push(Bytes::from(bytes));
            }
        }
    }
    for bytes in cases {
        let expected = String::from_utf8_lossy(&bytes);
        let prefix = std::str::from_utf8(&bytes).map_or_else(|error| error.valid_up_to(), str::len);
        for limit in [
            0,
            bytes.len(),
            prefix.saturating_sub(1),
            prefix,
            prefix + 1,
            expected.len().saturating_sub(1),
            expected.len(),
            expected.len() + 1,
        ] {
            let decoded = decode_http_call_body(&bytes, limit);
            assert_eq!(
                decoded.as_deref(),
                (expected.len() <= limit).then_some(expected.as_ref()),
                "input={bytes:?}, limit={limit}"
            );
        }
    }
}

#[test]
fn body_bytes_to_value_decodes_utf8() {
    let v = body_bytes_to_value(b"hello", 100).unwrap();
    assert_eq!(v, Value::Str("hello".into()));
}

#[test]
fn body_bytes_to_value_lossy_on_invalid_utf8() {
    // 0xFF is not valid UTF-8 → U+FFFD, not truncated.
    let v = body_bytes_to_value(&[0x61, 0xFF, 0x62], 100).unwrap();
    match v {
        Value::Str(s) => {
            assert!(s.starts_with('a') && s.ends_with('b'));
            assert!(s.contains('\u{FFFD}'));
        }
        other => panic!("expected Str, got {other:?}"),
    }
}

#[test]
fn body_bytes_to_value_rejects_oversize() {
    let err = body_bytes_to_value(b"toolong", 3).unwrap_err();
    assert!(matches!(err, RuntimeError::StringTooLong { .. }));
}

/// Mirrors the manual current-thread-runtime + `block_on(execute_safe(...))`
/// harness used throughout `dsl/tests/*` (e.g. `error_policy.rs`) —
/// `MockPluginSession` is not `Send`, so these tests use `#[test]`, not
/// `#[tokio::test]`.
#[test]
fn req_body_availability_failure_is_fixed_fail_close() {
    let b64 = compile_dsl_source("let b = req.body", &ValidationLimits::default()).expect("compile failed");
    let script = CompiledScript::deserialize_base64(&b64).expect("deserialize failed");
    let vm = Vm::new(script, VmLimits::default()).expect("vm creation failed");

    let mut session = MockPluginSession::new();
    session.expect_declares_body().return_const(Some(true));
    session
        .expect_request_body_available_len()
        .returning(|_| Err(BodyReadError::Unimplemented));
    session.expect_record_outcome().return_const(());

    let mut log = PluginLog::new("test");
    let rt = DslRuntime::default();
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(execute_safe(
            &vm,
            &mut session as &mut dyn PluginSession,
            &mut log,
            &DslErrorPolicy::Ignore,
            &rt,
        ));
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: Some(Bytes::from_static(b"Request body processing failed")),
        }
    );
}
