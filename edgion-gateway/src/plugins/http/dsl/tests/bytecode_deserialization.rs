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
    // "not_valid_base64" contains '_' (URL-safe alphabet) — STANDARD base64 rejects it
    // because '_' is not in the STANDARD alphabet.
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

// ── Validator rejects semantically-malicious bytecode ────────────────────────

use super::super::lang::validator::Validator;

fn make_script(code: Vec<OpCode>, local_count: u16) -> CompiledScript {
    // Manually constructed — bypasses the compiler so the validator's checks
    // are the only line of defense.
    CompiledScript {
        version: super::super::lang::bytecode::BYTECODE_VERSION,
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

use super::super::lang::bytecode::{BuiltinId, Constant};

#[test]
fn test_validator_rejects_setlocal_out_of_bounds() {
    // SetLocal(99) with local_count = 1 must be rejected — symmetric to the
    // GetLocal case; an adversary cannot declare 1 local but write to slot 99.
    let script = make_script(vec![OpCode::SetLocal(99), OpCode::ReturnNext], 1);
    let errors = Validator::new(ValidationLimits::default()).validate(&script);
    assert!(
        errors.iter().any(|e| e.message.contains("local slot 99")),
        "expected SetLocal out-of-bounds rejection, got: {:?}",
        errors
    );
}

#[test]
fn test_validator_rejects_loadconst_out_of_bounds() {
    // LoadConst(5) with an empty constant pool — the adversary cannot reference
    // an index that does not exist.
    let script = make_script(vec![OpCode::LoadConst(5), OpCode::ReturnNext], 0);
    let errors = Validator::new(ValidationLimits::default()).validate(&script);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("LoadConst") && e.message.contains("but pool size is")),
        "expected LoadConst out-of-bounds rejection, got: {:?}",
        errors
    );
}

#[test]
fn test_validator_rejects_loadconst_pointing_to_regex() {
    // LoadConst(0) where constants[0] is a Regex must be rejected — Regex
    // constants are not valid stack values; they are opaque references for the
    // regex opcodes only.
    let script = CompiledScript {
        version: 1,
        code: vec![OpCode::LoadConst(0), OpCode::ReturnNext],
        constants: vec![Constant::Regex("\\d+".to_string())],
        local_count: 0,
        max_loop_depth: 0,
        source: None,
        source_map: None,
    };
    let errors = Validator::new(ValidationLimits::default()).validate(&script);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("(Regex) which cannot be used as a value")),
        "expected LoadConst-Regex type rejection, got: {:?}",
        errors
    );
}

#[test]
fn test_validator_rejects_callbuiltin_wrong_argc() {
    // CallBuiltin(Substr, 2) — Substr is fixed-arity at 3; declaring 2 must be
    // caught by the validator before the script reaches the Gateway runtime.
    let script = make_script(vec![OpCode::CallBuiltin(BuiltinId::Substr, 2), OpCode::ReturnNext], 0);
    let errors = Validator::new(ValidationLimits::default()).validate(&script);
    assert!(
        errors.iter().any(
            |e| e.message.contains("CallBuiltin Substr") && e.message.contains("expects 3 arg(s), bytecode says 2")
        ),
        "expected CallBuiltin arity rejection, got: {:?}",
        errors
    );
}

#[test]
fn test_validator_rejects_makemap_exceeding_limit() {
    // MakeMap(5) with max_map_size = 4 must be rejected — an adversary cannot
    // inject an oversized map literal via the bytecode: config knob.
    let limits = ValidationLimits {
        max_map_size: 4,
        ..Default::default()
    };
    let script = make_script(vec![OpCode::MakeMap(5), OpCode::ReturnNext], 0);
    let errors = Validator::new(limits).validate(&script);
    assert!(
        errors.iter().any(|e| e
            .message
            .contains("MakeMap at instruction 0 requires 5 entries but limit is 4")),
        "expected MakeMap limit rejection, got: {:?}",
        errors
    );
}

#[test]
fn test_validator_rejects_regex_opcode_pointing_to_non_regex() {
    // Matches(0) where constants[0] is a plain Str must be rejected — the
    // validator's check_regex_ref helper guards all three regex opcodes.
    let script = CompiledScript {
        version: 1,
        code: vec![OpCode::Matches(0), OpCode::ReturnNext],
        constants: vec![Constant::Str("not-a-regex".to_string())],
        local_count: 0,
        max_loop_depth: 0,
        source: None,
        source_map: None,
    };
    let errors = Validator::new(ValidationLimits::default()).validate(&script);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("which is not a StreamingRegex")),
        "expected Matches non-Regex rejection, got: {:?}",
        errors
    );
}

#[test]
fn test_validator_rejects_regex_find_pointing_to_non_regex() {
    // Sibling of the Matches test — locks in the same check_regex_ref guard for
    // RegexFind so a future split of the helper does not silently drop coverage.
    let script = CompiledScript {
        version: 1,
        code: vec![OpCode::RegexFind(0), OpCode::ReturnNext],
        constants: vec![Constant::Str("not-a-regex".to_string())],
        local_count: 0,
        max_loop_depth: 0,
        source: None,
        source_map: None,
    };
    let errors = Validator::new(ValidationLimits::default()).validate(&script);
    assert!(
        errors.iter().any(|e| e.message.contains("which is not a Regex")),
        "expected RegexFind non-Regex rejection, got: {:?}",
        errors
    );
}

#[test]
fn test_validator_rejects_regex_replace_pointing_to_non_regex() {
    // Sibling of the Matches test — locks in the same check_regex_ref guard for
    // RegexReplace so a future split of the helper does not silently drop coverage.
    let script = CompiledScript {
        version: 1,
        code: vec![OpCode::RegexReplace(0), OpCode::ReturnNext],
        constants: vec![Constant::Str("not-a-regex".to_string())],
        local_count: 0,
        max_loop_depth: 0,
        source: None,
        source_map: None,
    };
    let errors = Validator::new(ValidationLimits::default()).validate(&script);
    assert!(
        errors.iter().any(|e| e.message.contains("which is not a Regex")),
        "expected RegexReplace non-Regex rejection, got: {:?}",
        errors
    );
}
