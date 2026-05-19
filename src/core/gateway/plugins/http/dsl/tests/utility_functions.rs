//! Tests for utility builtins: sha256, md5, regex, base64, url encode/decode, string ops, time, type conversions.

use super::*;
use parking_lot::Mutex;
use std::sync::Arc;

#[test]
fn test_sha256() {
    assert_sets_request_header(
        r#"req.set_header("X-Out", sha256("hello"))"#,
        "X-Out",
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
    );
}

#[test]
fn test_md5() {
    assert_sets_request_header(
        r#"req.set_header("X-Out", md5("hello"))"#,
        "X-Out",
        "5d41402abc4b2a76b9719d911017c592",
    );
}

#[test]
fn test_regex_find_match() {
    assert_sets_request_header(
        "let v = regex_find(\"foo-123\", \"\\\\d+\")\nif v != nil { req.set_header(\"X-Out\", v) }",
        "X-Out",
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
    assert_sets_request_header(
        r#"req.set_header("X-Out", regex_replace("abc-def", "-", "_"))"#,
        "X-Out",
        "abc_def",
    );
}

#[test]
fn test_regex_replace_all() {
    assert_sets_request_header(
        r#"req.set_header("X-Out", regex_replace("a-b-c", "-", "/"))"#,
        "X-Out",
        "a/b/c",
    );
}

// regex_find / regex_replace — dynamic (variable) pattern is rejected at compile time
#[test]
fn test_regex_find_dynamic_pattern_compile_error() {
    let result = compile_dsl_source(
        r#"let p = "\\d+"
let v = regex_find("foo-123", p)"#,
        &ValidationLimits::default(),
    );
    assert!(result.is_err(), "dynamic pattern should fail at compile time");
    let errs = result.unwrap_err();
    assert!(
        errs.iter().any(|e| e.contains("string literal")),
        "error should mention string literal requirement, got: {:?}",
        errs
    );
}

#[test]
fn test_regex_replace_dynamic_pattern_compile_error() {
    let result = compile_dsl_source(
        r#"let p = "-"
let v = regex_replace("a-b-c", p, "/")"#,
        &ValidationLimits::default(),
    );
    assert!(result.is_err(), "dynamic pattern should fail at compile time");
    let errs = result.unwrap_err();
    assert!(
        errs.iter().any(|e| e.contains("string literal")),
        "error should mention string literal requirement, got: {:?}",
        errs
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

// Literal pattern emits dedicated OpCode::RegexFind / RegexReplace
#[test]
fn test_regex_find_literal_emits_dedicated_opcode() {
    use super::super::lang::bytecode::{CompiledScript, OpCode};
    let b64 = compile_dsl_source(r#"let v = regex_find("foo-123", "\\d+")"#, &ValidationLimits::default())
        .expect("compile ok");
    let script = CompiledScript::deserialize_base64(&b64).expect("deserialize ok");
    assert!(
        script.code.iter().any(|op| matches!(op, OpCode::RegexFind(_))),
        "literal pattern should emit OpCode::RegexFind"
    );
}

#[test]
fn test_regex_replace_literal_emits_dedicated_opcode() {
    use super::super::lang::bytecode::{CompiledScript, OpCode};
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
}

// regex_replace with no match returns the original string unchanged (Cow::Borrowed path)
#[test]
fn test_regex_replace_no_match_returns_original() {
    assert_sets_request_header(
        r#"req.set_header("X-Out", regex_replace("hello", "\\d+", "X"))"#,
        "X-Out",
        "hello",
    );
}

// regex_replace with capture group references ($1, $2, …)
#[test]
fn test_regex_replace_capture_groups() {
    // Rearrange YYYY-MM-DD to DD/MM/YYYY using three capture groups
    assert_sets_request_header(
        r#"req.set_header("X-Out", regex_replace("2026-05-15", "(\\d{4})-(\\d{2})-(\\d{2})", "$3/$2/$1"))"#,
        "X-Out",
        "15/05/2026",
    );
}

// regex_replace with nil subject returns nil (nil-friendly, consistent with regex_find)
#[test]
fn test_regex_replace_nil_subject_returns_nil() {
    let mut session = MockPluginSession::new();
    session
        .expect_header_value()
        .withf(|h| h == "Missing")
        .returning(|_| None);
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-Out" && v == "nil-returned")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        r#"let v = regex_replace(req.header("Missing"), "\\d+", "X")
if v == nil { req.set_header("X-Out", "nil-returned") }"#,
        session,
    );
    assert_eq!(result, good_next());
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
    assert_sets_request_header(
        r#"req.set_header("X-Out", base64_encode("hello"))"#,
        "X-Out",
        "aGVsbG8=",
    );
}

#[test]
fn test_base64_decode() {
    assert_sets_request_header(
        r#"req.set_header("X-Out", base64_decode("aGVsbG8="))"#,
        "X-Out",
        "hello",
    );
}

#[test]
fn test_url_encode() {
    assert_sets_request_header(r#"req.set_header("X-Out", url_encode("a b+c"))"#, "X-Out", "a%20b%2Bc");
}

#[test]
fn test_url_decode() {
    assert_sets_request_header(r#"req.set_header("X-Out", url_decode("a%20b"))"#, "X-Out", "a b");
}

// base64_decode invalid input returns nil — matches hex_decode contract.
#[test]
fn test_base64_decode_invalid_returns_nil() {
    let source = r#"
        let val = base64_decode("not_base64!!!")
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

// url_decode invalid input preserves literal — unlike base64_decode/hex_decode, an invalid %XX
// sequence is passed through as-is (per while-loop fall-through in BuiltinId::UrlDecode).
#[test]
fn test_url_decode_invalid_percent_preserves_literal() {
    assert_sets_request_header(r#"req.set_header("X-Out", url_decode("%ZZ"))"#, "X-Out", "%ZZ");
}

#[test]
fn test_to_upper() {
    assert_sets_request_header(r#"req.set_header("X-Out", to_upper("hello"))"#, "X-Out", "HELLO");
}

#[test]
fn test_to_lower() {
    assert_sets_request_header(r#"req.set_header("X-Out", to_lower("HELLO"))"#, "X-Out", "hello");
}

#[test]
fn test_substr() {
    assert_sets_request_header(r#"req.set_header("X-Out", substr("hello", 1, 3))"#, "X-Out", "el");
}

// substr OOB clamp semantics — start/end beyond char_count yield "" rather than ApiError;
// (*n).max(0) clamps negatives. Lock in current behavior so future refactors don't drift.
#[test]
fn test_substr_oob_returns_empty() {
    assert_sets_request_header(r#"req.set_header("X-Out", substr("abc", 10, 3))"#, "X-Out", "");
}

#[test]
fn test_substr_negative_start_clamps_to_zero() {
    assert_sets_request_header(r#"req.set_header("X-Out", substr("abc", -1, 2))"#, "X-Out", "ab");
}

#[test]
fn test_len() {
    assert_sets_request_header(r#"req.set_header("X-Out", to_str(len("abc")))"#, "X-Out", "3");
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
    assert_sets_request_header(r#"req.set_header("X-Out", to_str(to_int("42")))"#, "X-Out", "42");
}

// to_int semantics for unparseable/out-of-range strings: all return nil (never TypeError or
// IntegerOverflow). Locks in the unwrap_or(Value::Nil) behavior in BuiltinId::ToInt.
#[test]
fn test_to_int_unparseable_returns_nil() {
    assert_sets_request_header(
        r#"let v = to_int("abc")
if v == nil { req.set_header("X-Out", "nil") }"#,
        "X-Out",
        "nil",
    );
}

#[test]
fn test_to_int_empty_string_returns_nil() {
    assert_sets_request_header(
        r#"let v = to_int("")
if v == nil { req.set_header("X-Out", "nil") }"#,
        "X-Out",
        "nil",
    );
}

#[test]
fn test_to_int_overflow_returns_nil() {
    assert_sets_request_header(
        r#"let v = to_int("99999999999999999999")
if v == nil { req.set_header("X-Out", "nil") }"#,
        "X-Out",
        "nil",
    );
}

#[test]
fn test_to_str() {
    assert_sets_request_header(r#"req.set_header("X-Out", to_str(42))"#, "X-Out", "42");
}

#[test]
fn test_log_returns_good_next() {
    let session = MockPluginSession::new();
    let result = run(r#"log("hello")"#, session);
    assert_eq!(result, good_next());
}

// url_decode("%") — single byte, i+2 < bytes.len() is 0+2<1 false → fall-through preserves "%".
#[test]
fn test_url_decode_single_percent_preserves_literal() {
    assert_sets_request_header(r#"req.set_header("X-Out", url_decode("%"))"#, "X-Out", "%");
}

// url_decode("%Z") — two bytes, i+2 < bytes.len() is 0+2<2 false → fall-through preserves "%Z".
#[test]
fn test_url_decode_percent_one_char_preserves_literal() {
    assert_sets_request_header(r#"req.set_header("X-Out", url_decode("%Z"))"#, "X-Out", "%Z");
}

// substr("", 0, 1) — empty string, both indices clamp to 0 → "".
#[test]
fn test_substr_empty_input_returns_empty() {
    assert_sets_request_header(r#"req.set_header("X-Out", substr("", 0, 1))"#, "X-Out", "");
}

// to_int("  42  ") — Rust parse::<i64> rejects surrounding whitespace → nil.
#[test]
fn test_to_int_surrounding_whitespace_returns_nil() {
    assert_sets_request_header(
        r#"let v = to_int("  42  ")
if v == nil { req.set_header("X-Out", "nil") }"#,
        "X-Out",
        "nil",
    );
}

// to_int("42abc") — trailing garbage causes parse failure → nil.
#[test]
fn test_to_int_trailing_garbage_returns_nil() {
    assert_sets_request_header(
        r#"let v = to_int("42abc")
if v == nil { req.set_header("X-Out", "nil") }"#,
        "X-Out",
        "nil",
    );
}

// to_int("0x10") — parse::<i64> does not accept hex prefix → nil.
#[test]
fn test_to_int_hex_prefix_returns_nil() {
    assert_sets_request_header(
        r#"let v = to_int("0x10")
if v == nil { req.set_header("X-Out", "nil") }"#,
        "X-Out",
        "nil",
    );
}
