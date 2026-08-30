//! Tests that the VM's defensive error paths fire correctly when hand-crafted
//! bytecode bypasses the Validator.

use super::super::lang::bytecode::{Constant, JsonSelectEntry, OpCode, BYTECODE_VERSION};
use super::*;

#[test]
fn test_vm_handcrafted_body_hmac_stack_underflow_is_safe() {
    let script = CompiledScript {
        version: BYTECODE_VERSION,
        code: vec![OpCode::BodyHmacSha256, OpCode::ReturnNext],
        constants: vec![],
        local_count: 0,
        max_loop_depth: 0,
        source: None,
        source_map: None,
    };
    let vm = Vm::new(script, VmLimits::default()).expect("vm creation failed");
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("test");
    let runtime = DslRuntime::default();
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(vm.execute(&mut session as &mut dyn PluginSession, &mut log, &runtime));
    assert!(matches!(
        result,
        Err(RuntimeError::Internal { ref message }) if message == "stack underflow"
    ));
}

/// A hand-crafted script with LoadConst(0) pointing at a Regex constant bypasses
/// the Validator and reaches the VM. The VM must return RuntimeError::Internal,
/// not silently push Nil.
#[test]
fn test_vm_loadconst_regex_returns_internal_error() {
    let script = CompiledScript {
        version: BYTECODE_VERSION,
        code: vec![OpCode::LoadConst(0), OpCode::ReturnNext],
        constants: vec![Constant::Regex("\\d+".to_string())],
        local_count: 0,
        max_loop_depth: 0,
        source: None,
        source_map: None,
    };
    let vm = Vm::new(script, VmLimits::default()).expect("vm creation failed");
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("test");
    let rt = DslRuntime::default();
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(vm.execute(&mut session as &mut dyn PluginSession, &mut log, &rt));
    let err = result.expect_err("expected RuntimeError::Internal for LoadConst(Regex), got Ok");
    assert!(
        err.to_string()
            .contains("LoadConst at pc 0 references constant 0 (Regex) which cannot be used as a value"),
        "expected Regex-as-value error, got: {}",
        err
    );
}

/// Hand-crafted scripts with an out-of-range regex_cache index for Matches / RegexFind /
/// RegexReplace must return RuntimeError::Internal, not silently return false / Nil / original.
/// (Unreachable through the compile+validate pipeline; exercises the defensive guard only.)
#[test]
fn test_vm_matches_bad_idx_returns_internal_error() {
    let script = CompiledScript {
        version: BYTECODE_VERSION,
        // LoadConst(0) pushes "hello" onto the stack; Matches(5) has no regex_cache[5].
        code: vec![OpCode::LoadConst(0), OpCode::Matches(5), OpCode::ReturnNext],
        constants: vec![Constant::Str("hello".to_string())],
        local_count: 0,
        max_loop_depth: 0,
        source: None,
        source_map: None,
    };
    let vm = Vm::new(script, VmLimits::default()).expect("vm creation failed");
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("test");
    let rt = DslRuntime::default();
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(vm.execute(&mut session as &mut dyn PluginSession, &mut log, &rt));
    let err = result.expect_err("expected RuntimeError::Internal for Matches(bad_idx), got Ok");
    assert!(
        err.to_string().contains("regex_cache[5] missing or invalid"),
        "expected regex_cache missing error, got: {}",
        err
    );
}

#[test]
fn test_vm_regex_find_bad_idx_returns_internal_error() {
    let script = CompiledScript {
        version: BYTECODE_VERSION,
        code: vec![OpCode::LoadConst(0), OpCode::RegexFind(5), OpCode::ReturnNext],
        constants: vec![Constant::Str("hello".to_string())],
        local_count: 0,
        max_loop_depth: 0,
        source: None,
        source_map: None,
    };
    let vm = Vm::new(script, VmLimits::default()).expect("vm creation failed");
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("test");
    let rt = DslRuntime::default();
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(vm.execute(&mut session as &mut dyn PluginSession, &mut log, &rt));
    let err = result.expect_err("expected RuntimeError::Internal for RegexFind(bad_idx), got Ok");
    assert!(
        err.to_string().contains("regex_cache[5] missing or invalid"),
        "expected regex_cache missing error, got: {}",
        err
    );
}

#[test]
fn test_vm_regex_replace_bad_idx_returns_internal_error() {
    let script = CompiledScript {
        version: BYTECODE_VERSION,
        // Push subject then replacement; RegexReplace(5) pops replacement then subject.
        code: vec![
            OpCode::LoadConst(0),
            OpCode::LoadConst(0),
            OpCode::RegexReplace(5),
            OpCode::ReturnNext,
        ],
        constants: vec![Constant::Str("hello".to_string())],
        local_count: 0,
        max_loop_depth: 0,
        source: None,
        source_map: None,
    };
    let vm = Vm::new(script, VmLimits::default()).expect("vm creation failed");
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("test");
    let rt = DslRuntime::default();
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(vm.execute(&mut session as &mut dyn PluginSession, &mut log, &rt));
    let err = result.expect_err("expected RuntimeError::Internal for RegexReplace(bad_idx), got Ok");
    assert!(
        err.to_string().contains("regex_cache[5] missing or invalid"),
        "expected regex_cache missing error, got: {}",
        err
    );
}

/// A hand-crafted script with LoadConst(5) into an empty pool bypasses the Validator
/// and reaches the VM. The VM must return RuntimeError::Internal (out-of-bounds guard),
/// not panic or silently push Nil.
#[test]
fn test_vm_loadconst_out_of_bounds_returns_internal_error() {
    let script = CompiledScript {
        version: BYTECODE_VERSION,
        code: vec![OpCode::LoadConst(5), OpCode::ReturnNext],
        constants: vec![],
        local_count: 0,
        max_loop_depth: 0,
        source: None,
        source_map: None,
    };
    let vm = Vm::new(script, VmLimits::default()).expect("vm creation failed");
    let mut session = MockPluginSession::new();
    let mut log = PluginLog::new("test");
    let rt = DslRuntime::default();
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(vm.execute(&mut session as &mut dyn PluginSession, &mut log, &rt));
    let err = result.expect_err("expected RuntimeError::Internal for out-of-bounds LoadConst, got Ok");
    assert!(
        err.to_string().contains("constant index 5 out of bounds"),
        "expected out-of-bounds error, got: {}",
        err
    );
}

#[test]
fn test_vm_rejects_malformed_json_select_plans_without_validator() {
    for (entries, expected) in [
        (vec![], "validated JSON select plan is empty"),
        (
            vec![
                JsonSelectEntry {
                    name: "duplicate".to_string(),
                    path: "$.first".to_string(),
                },
                JsonSelectEntry {
                    name: "duplicate".to_string(),
                    path: "$.second".to_string(),
                },
            ],
            "validated JSON select output name is duplicated",
        ),
        (
            vec![JsonSelectEntry {
                name: "value".to_string(),
                path: "$.*".to_string(),
            }],
            "invalid validated JSON select path",
        ),
    ] {
        let script = CompiledScript {
            version: BYTECODE_VERSION,
            code: vec![OpCode::JsonSelect(0), OpCode::ReturnNext],
            constants: vec![Constant::JsonSelect(entries)],
            local_count: 0,
            max_loop_depth: 0,
            source: None,
            source_map: None,
        };
        let error = match Vm::new(script, VmLimits::default()) {
            Ok(_) => panic!("malformed JSON select plan unexpectedly initialized"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains(expected),
            "expected {expected:?}, got: {error}"
        );
    }
}
