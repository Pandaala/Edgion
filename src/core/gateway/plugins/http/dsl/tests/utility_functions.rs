//! Tests for utility builtins: sha256, md5, regex, base64, url encode/decode, string ops, time, type conversions.

use super::*;
use parking_lot::Mutex;
use std::sync::Arc;

fn assert_output(source: &str, expected: &str) {
    let expected = expected.to_string();
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(move |h, v| h == "X-Out" && v == expected)
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

#[test]
fn test_sha256() {
    assert_output(
        r#"req.set_header("X-Out", sha256("hello"))"#,
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
    );
}

#[test]
fn test_md5() {
    assert_output(
        r#"req.set_header("X-Out", md5("hello"))"#,
        "5d41402abc4b2a76b9719d911017c592",
    );
}

#[test]
fn test_regex_find_match() {
    assert_output(
        "let v = regex_find(\"foo-123\", \"\\\\d+\")\nif v != nil { req.set_header(\"X-Out\", v) }",
        "123",
    );
}

#[test]
fn test_regex_find_no_match() {
    let session = MockPluginSession::new();
    let result = run(
        "let v = regex_find(\"foo\", \"\\\\d+\")\nif v != nil { req.set_header(\"X-Out\", v) }",
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_regex_replace_single() {
    assert_output(
        r#"req.set_header("X-Out", regex_replace("abc-def", "-", "_"))"#,
        "abc_def",
    );
}

#[test]
fn test_regex_replace_all() {
    assert_output(r#"req.set_header("X-Out", regex_replace("a-b-c", "-", "/"))"#, "a/b/c");
}

// regex_find / regex_replace — dynamic (variable) pattern falls back to CallBuiltin path
#[test]
fn test_regex_find_dynamic_pattern() {
    assert_output(
        r#"let p = "\\d+"
let v = regex_find("foo-123", p)
if v != nil { req.set_header("X-Out", v) }"#,
        "123",
    );
}

#[test]
fn test_regex_replace_dynamic_pattern() {
    assert_output(
        r#"let p = "-"
req.set_header("X-Out", regex_replace("a-b-c", p, "/"))"#,
        "a/b/c",
    );
}

// Same literal pattern used twice → only one Regex in constant pool
#[test]
fn test_regex_literal_dedup() {
    use super::super::lang::bytecode::{CompiledScript, Constant};
    use super::super::lang::validator::{compile_dsl_source, ValidationLimits};
    let source = r#"
let a = regex_find("foo-123", "\\d+")
let b = regex_find("bar-456", "\\d+")
"#;
    let b64 = compile_dsl_source(source, &ValidationLimits::default()).expect("compile ok");
    let script = CompiledScript::deserialize_base64(&b64).expect("deserialize ok");
    let regex_count = script
        .constants
        .iter()
        .filter(|c| matches!(c, Constant::Regex(_)))
        .count();
    assert_eq!(regex_count, 1, "same pattern should deduplicate to one constant");
}

// Nil input to regex_find returns Nil (nil-friendly, consistent with Matches)
#[test]
fn test_regex_find_nil_input_returns_nil() {
    let mut session = MockPluginSession::new();
    // req.header("Missing") returns None → nil; regex_find on nil must return nil, not match "nil"
    session
        .expect_header_value()
        .withf(|h| h == "Missing")
        .returning(|_| None);
    let result = run(
        r#"let v = regex_find(req.header("Missing"), "nil")
if v != nil { req.set_header("X-Out", "matched") }"#,
        session,
    );
    assert_eq!(result, good_next());
}

// Invalid literal pattern → CompileError at compile time
#[test]
fn test_regex_find_invalid_literal_pattern_compile_error() {
    let result = compile_dsl_source(r#"let v = regex_find("foo", "[")"#, &ValidationLimits::default());
    assert!(result.is_err(), "invalid regex literal should fail at compile time");
    let errs = result.unwrap_err();
    assert!(
        errs.iter().any(|e| e.contains("invalid regex")),
        "error should mention invalid regex"
    );
}

// Literal pattern emits dedicated OpCode::RegexFind / RegexReplace, not CallBuiltin
#[test]
fn test_regex_find_literal_emits_dedicated_opcode() {
    use super::super::lang::bytecode::{BuiltinId, CompiledScript, OpCode};
    let b64 = compile_dsl_source(r#"let v = regex_find("foo-123", "\\d+")"#, &ValidationLimits::default())
        .expect("compile ok");
    let script = CompiledScript::deserialize_base64(&b64).expect("deserialize ok");
    assert!(
        script.code.iter().any(|op| matches!(op, OpCode::RegexFind(_))),
        "literal pattern should emit OpCode::RegexFind"
    );
    assert!(
        !script
            .code
            .iter()
            .any(|op| matches!(op, OpCode::CallBuiltin(BuiltinId::RegexFind, _))),
        "literal pattern should NOT emit CallBuiltin(RegexFind)"
    );
}

#[test]
fn test_regex_replace_literal_emits_dedicated_opcode() {
    use super::super::lang::bytecode::{BuiltinId, CompiledScript, OpCode};
    let b64 = compile_dsl_source(
        r#"let v = regex_replace("a-b", "-", "/")"#,
        &ValidationLimits::default(),
    )
    .expect("compile ok");
    let script = CompiledScript::deserialize_base64(&b64).expect("deserialize ok");
    assert!(
        script.code.iter().any(|op| matches!(op, OpCode::RegexReplace(_))),
        "literal pattern should emit OpCode::RegexReplace"
    );
    assert!(
        !script
            .code
            .iter()
            .any(|op| matches!(op, OpCode::CallBuiltin(BuiltinId::RegexReplace, _))),
        "literal pattern should NOT emit CallBuiltin(RegexReplace)"
    );
}

// regex_replace with no match returns the original string unchanged (Cow::Borrowed path)
#[test]
fn test_regex_replace_no_match_returns_original() {
    assert_output(
        r#"req.set_header("X-Out", regex_replace("hello", "\\d+", "X"))"#,
        "hello",
    );
}

// regex_find / regex_replace result exceeding max_string_len → runtime error
#[test]
fn test_regex_find_result_too_long() {
    let session = MockPluginSession::new();
    let limits = VmLimits {
        max_string_len: 3,
        ..Default::default()
    };
    // "abcd" matches [a-z]+ fully; result length 4 > limit 3
    let result = run_with_limits(r#"let v = regex_find("abcd", "[a-z]+")"#, session, limits);
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
}

#[test]
fn test_regex_replace_result_too_long() {
    let session = MockPluginSession::new();
    let limits = VmLimits {
        max_string_len: 3,
        ..Default::default()
    };
    // "a-b" → "aXXb" (length 4) > limit 3
    let result = run_with_limits(r#"let v = regex_replace("a-b", "-", "XX")"#, session, limits);
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
}

#[test]
fn test_base64_encode() {
    assert_output(r#"req.set_header("X-Out", base64_encode("hello"))"#, "aGVsbG8=");
}

#[test]
fn test_base64_decode() {
    assert_output(r#"req.set_header("X-Out", base64_decode("aGVsbG8="))"#, "hello");
}

#[test]
fn test_url_encode() {
    assert_output(r#"req.set_header("X-Out", url_encode("a b+c"))"#, "a%20b%2Bc");
}

#[test]
fn test_url_decode() {
    assert_output(r#"req.set_header("X-Out", url_decode("a%20b"))"#, "a b");
}

#[test]
fn test_to_upper() {
    assert_output(r#"req.set_header("X-Out", to_upper("hello"))"#, "HELLO");
}

#[test]
fn test_to_lower() {
    assert_output(r#"req.set_header("X-Out", to_lower("HELLO"))"#, "hello");
}

#[test]
fn test_substr() {
    assert_output(r#"req.set_header("X-Out", substr("hello", 1, 3))"#, "el");
}

#[test]
fn test_len() {
    assert_output(r#"req.set_header("X-Out", to_str(len("abc")))"#, "3");
}

#[test]
fn test_time_now() {
    let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let captured_clone = captured.clone();
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|h, _| h == "X-T")
        .times(1)
        .returning(move |_, v| {
            *captured_clone.lock() = Some(v.to_string());
            Ok(())
        });
    let result = run(r#"req.set_header("X-T", to_str(time_now()))"#, session);
    assert_eq!(result, good_next());
    let val = captured.lock().clone().unwrap();
    let ts: i64 = val.parse().expect("time_now should produce a parseable integer");
    assert!(ts > 0, "time_now() should return a positive timestamp, got {}", ts);
}

#[test]
fn test_to_int() {
    assert_output(r#"req.set_header("X-Out", to_str(to_int("42")))"#, "42");
}

#[test]
fn test_to_str() {
    assert_output(r#"req.set_header("X-Out", to_str(42))"#, "42");
}

#[test]
fn test_log_returns_good_next() {
    let session = MockPluginSession::new();
    let result = run(r#"log("hello")"#, session);
    assert_eq!(result, good_next());
}
