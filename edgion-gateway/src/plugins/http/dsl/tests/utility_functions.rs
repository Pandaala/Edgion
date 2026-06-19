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

// sha256 / md5 result must honor max_string_len like every other string builtin (F-005)
#[test]
fn test_sha256_result_too_long() {
    let session = MockPluginSession::new();
    let limits = VmLimits {
        max_string_len: 63,
        ..Default::default()
    };
    // SHA-256 hex output is 64 chars > limit 63
    let result = run_with_limits(r#"let v = sha256("hello")"#, session, limits);
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
}

#[test]
fn test_md5_result_too_long() {
    let session = MockPluginSession::new();
    let limits = VmLimits {
        max_string_len: 31,
        ..Default::default()
    };
    // MD5 hex output is 32 chars > limit 31
    let result = run_with_limits(r#"let v = md5("hello")"#, session, limits);
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

// url_encode result exceeding max_string_len → runtime error.
// Encoder builds incrementally and bails inside the per-char loop as soon as
// `encoded.len() > max_string_len`, instead of `collect()`-then-check.
#[test]
fn test_url_encode_result_too_long() {
    let session = MockPluginSession::new();
    let limits = VmLimits {
        max_string_len: 5,
        ..Default::default()
    };
    // "a b c" encodes to "a%20b%20c" (length 9) > limit 5 — the in-loop bail trips
    // mid-encoding once running length crosses 5.
    let result = run_with_limits(r#"let v = url_encode("a b c")"#, session, limits);
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
}

// A coarse `s.len() * 3 > max_string_len` pre-guard would false-reject this case.
// Encoded output equals input (all safe ASCII), so it MUST succeed even though
// `input.len() * 3 = 12 > limit = 8`. Locks the in-loop bail semantics against
// future "optimizations" to a length-only pre-check.
#[test]
fn test_url_encode_safe_ascii_within_limit_succeeds() {
    let limits = VmLimits {
        max_string_len: 8,
        ..Default::default()
    };
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(|n, v| n == "X-Out" && v == "abcdefgh")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run_with_limits(r#"req.set_header("X-Out", url_encode("abcdefgh"))"#, session, limits);
    assert_eq!(result, good_next());
}

#[test]
fn test_url_decode() {
    assert_sets_request_header(r#"req.set_header("X-Out", url_decode("a%20b"))"#, "X-Out", "a b");
}

// `url_decode` uses application/x-www-form-urlencoded semantics: a literal `+` decodes to a
// space. This is intentional and documented in api-reference.md §url_decode (Semantics +
// Security warning) and in the BuiltinId::UrlDecode source comment. Mirrors the same
// semantics used by edgion_resources::uri_utils::url_decode at the routing layer. See
// skills/04-review/dsl-builtins/url-decode-form-semantics.md for the design lock.
#[test]
fn test_url_decode_plus_decodes_to_space_form_semantics() {
    assert_sets_request_header(r#"req.set_header("X-Out", url_decode("a+b"))"#, "X-Out", "a b");
}

// Mixed `%XX` + `+` exercises both branches of the decoder in one input.
#[test]
fn test_url_decode_mixed_percent_and_plus_form_semantics() {
    assert_sets_request_header(r#"req.set_header("X-Out", url_decode("a+b%20c"))"#, "X-Out", "a b c");
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

// Locks the documented `len()` contract: Unicode character count, NOT UTF-8 byte
// count. "héllo" is 5 chars but 6 UTF-8 bytes (the `é` is a 2-byte sequence) —
// a regression that switched to `s.len()` would surface here as 6. This guard is
// paired with the byte-unit note on the `len` entry in dsl/api-reference.md.
#[test]
fn test_len_multibyte_returns_char_count_not_byte_count() {
    assert_sets_request_header(r#"req.set_header("X-Out", to_str(len("héllo")))"#, "X-Out", "5");
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
