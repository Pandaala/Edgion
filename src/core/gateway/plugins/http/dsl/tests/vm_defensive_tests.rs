//! Tests that the VM's defensive error paths fire correctly when hand-crafted
//! bytecode bypasses the Validator.

use super::super::lang::bytecode::{Constant, OpCode, BYTECODE_VERSION};
use super::*;

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
