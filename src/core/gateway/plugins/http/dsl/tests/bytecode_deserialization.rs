//! Negative tests for `CompiledScript::deserialize_base64`.
//!
//! Each test exercises one of the three error branches in `lang/bytecode.rs`:
//! base64 decode failure, JSON deserialize failure, and unsupported version.
//! The version-mismatch path also has an in-crate sibling test
//! (`bytecode.rs::tests::test_reject_unknown_version`); this file covers the
//! public test surface.

use super::*;

#[test]
fn test_deserialize_invalid_base64() {
    // "not_valid_base64" contains '_' (URL-safe alphabet) — STANDARD base64 rejects it,
    // and even allowing for permissive decoders the length (16) is not a multiple of 4
    // without padding. STANDARD::decode returns Err either way.
    let err = CompiledScript::deserialize_base64("not_valid_base64").expect_err("invalid base64 must fail");
    assert!(err.contains("base64"), "expected base64 error, got: {}", err);
}

#[test]
fn test_deserialize_truncated_json() {
    use base64::Engine;
    // Valid base64 wrapping invalid (truncated) JSON. Decodes fine, fails at serde_json.
    let truncated = br#"{"version":1,"code":["#;
    let encoded = base64::engine::general_purpose::STANDARD.encode(truncated);
    let err = CompiledScript::deserialize_base64(&encoded).expect_err("truncated JSON must fail");
    assert!(err.contains("deserialize"), "expected deserialize error, got: {}", err);
}

#[test]
fn test_deserialize_unsupported_version() {
    use base64::Engine;
    // Use version 255 — distinct from BYTECODE_VERSION (1), LEGACY_BYTECODE_VERSION (0),
    // and the inline test's value (999), so each test exercises an independent input.
    let payload = serde_json::json!({
        "version": 255,
        "code": ["ReturnNext"],
        "constants": [],
        "local_count": 0,
        "max_loop_depth": 0
    });
    let bytes = serde_json::to_vec(&payload).expect("json encode");
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    let err = CompiledScript::deserialize_base64(&encoded).expect_err("unsupported version must fail");
    assert!(
        err.contains("version"),
        "expected unsupported version error, got: {}",
        err
    );
}

// ── Validator rejects semantically-malicious bytecode (#14) ─────────────────

use super::super::lang::validator::Validator;

fn make_script(code: Vec<OpCode>, local_count: u16) -> CompiledScript {
    // Manually constructed — bypasses the compiler so the validator's checks
    // are the only line of defense.
    CompiledScript {
        version: 1,
        code,
        constants: vec![],
        local_count,
        max_loop_depth: 0,
        source: None,
        source_map: None,
    }
}

#[test]
fn test_validator_rejects_jump_out_of_bounds() {
    // Jump(i32::MAX) from PC 0 yields target ~ 2^31, far beyond a 1-instruction body.
    let script = make_script(vec![OpCode::Jump(i32::MAX), OpCode::ReturnNext], 0);
    let errors = Validator::new(ValidationLimits::default()).validate(&script);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("jump") && e.message.contains("out of bounds")),
        "expected jump-out-of-bounds rejection, got: {:?}",
        errors
    );
}

#[test]
fn test_validator_rejects_local_slot_out_of_bounds() {
    // GetLocal(99) with local_count = 1 must be rejected — adversary cannot
    // declare 1 local but read slot 99.
    let script = make_script(vec![OpCode::GetLocal(99), OpCode::ReturnNext], 1);
    let errors = Validator::new(ValidationLimits::default()).validate(&script);
    assert!(
        errors.iter().any(|e| e.message.contains("local slot 99")),
        "expected local-slot-out-of-bounds rejection, got: {:?}",
        errors
    );
}
