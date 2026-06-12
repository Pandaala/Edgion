//! Unit tests for EdgionDSL VM execution semantics.
//!
//! Uses MockPluginSession to validate behaviour without real HTTP connections.
//! MockPluginSession is not Send — all tests use #[test], not #[tokio::test].

pub(super) use super::lang::bytecode::{CompiledScript, OpCode};
pub(super) use super::lang::error::RuntimeError;
pub(super) use super::lang::validator::{compile_dsl_source, ValidationLimits};
pub(super) use super::lang::vm::{execute_safe, DslErrorPolicy, Vm, VmLimits};
pub(super) use super::runtime::DslRuntime;
pub(super) use crate::plugins::runtime::traits::session::{MockPluginSession, PluginSession};
pub(super) use crate::plugins::runtime::PluginLog;
pub(super) use edgion_resources::filters::PluginRunningResult;

mod api_errors;
mod bytecode_deserialization;
mod compile_errors;
mod control_flow;
mod ctx_builtins;
mod decision_log_tests;
mod error_policy;
mod grpc_error_tests;
mod http_call_tests;
mod p3_utility_tests;
mod property_bytecode;
mod range_and_string_ops;
mod req_metadata_tests;
mod request_builtins;
mod request_mutation;
mod resp_p2_tests;
mod resp_set_status_tests;
mod response_builtins;
mod runtime_errors;
mod sandbox;
mod terminate_tests;
mod utility_functions;
mod vm_defensive_tests;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub(super) fn make_vm(source: &str) -> Vm {
    let b64 =
        compile_dsl_source(source, &ValidationLimits::default()).unwrap_or_else(|e| panic!("compile failed: {:?}", e));
    let script = CompiledScript::deserialize_base64(&b64).expect("deserialize failed");
    Vm::new(script, VmLimits::default()).expect("vm creation failed")
}

pub(super) fn make_vm_with_limits(source: &str, limits: VmLimits) -> Vm {
    let b64 =
        compile_dsl_source(source, &ValidationLimits::default()).unwrap_or_else(|e| panic!("compile failed: {:?}", e));
    let script = CompiledScript::deserialize_base64(&b64).expect("deserialize failed");
    Vm::new(script, limits).expect("vm creation failed")
}

pub(super) fn run(source: &str, session: MockPluginSession) -> PluginRunningResult {
    let vm = make_vm(source);
    let mut log = PluginLog::new("test");
    let mut s = session;
    let rt = DslRuntime::default();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(execute_safe(
            &vm,
            &mut s as &mut dyn PluginSession,
            &mut log,
            &DslErrorPolicy::Deny,
            &rt,
        ))
}

pub(super) fn run_with_limits(source: &str, session: MockPluginSession, limits: VmLimits) -> PluginRunningResult {
    let vm = make_vm_with_limits(source, limits);
    let mut log = PluginLog::new("test");
    let mut s = session;
    let rt = DslRuntime::default();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(execute_safe(
            &vm,
            &mut s as &mut dyn PluginSession,
            &mut log,
            &DslErrorPolicy::Deny,
            &rt,
        ))
}

pub(super) fn run_with_runtime(source: &str, session: MockPluginSession, runtime: DslRuntime) -> PluginRunningResult {
    let vm = make_vm(source);
    let mut log = PluginLog::new("test");
    let mut s = session;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(execute_safe(
            &vm,
            &mut s as &mut dyn PluginSession,
            &mut log,
            &DslErrorPolicy::Deny,
            &runtime,
        ))
}

/// Synchronous wrapper for execute_safe — for #[test] (non-async) contexts.
pub(super) fn run_vm_policy(vm: &Vm, mut session: MockPluginSession, policy: &DslErrorPolicy) -> PluginRunningResult {
    let mut log = PluginLog::new("test");
    let rt_ctx = DslRuntime::default();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(execute_safe(
            vm,
            &mut session as &mut dyn PluginSession,
            &mut log,
            policy,
            &rt_ctx,
        ))
}

/// Like run_vm_policy but also returns the log for assertions on log content.
pub(super) fn run_vm_policy_log(
    vm: &Vm,
    mut session: MockPluginSession,
    policy: &DslErrorPolicy,
) -> (PluginRunningResult, PluginLog) {
    let mut log = PluginLog::new("test");
    let rt_ctx = DslRuntime::default();
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(execute_safe(
            vm,
            &mut session as &mut dyn PluginSession,
            &mut log,
            policy,
            &rt_ctx,
        ));
    (result, log)
}

/// Run source and expect a RuntimeError — bypasses execute_safe + DslErrorPolicy so
/// callers can assert the exact variant via `matches!`.
pub(super) fn run_expect_runtime_err(source: &str, session: MockPluginSession) -> RuntimeError {
    let vm = make_vm(source);
    let mut log = PluginLog::new("test");
    let mut s = session;
    let rt = DslRuntime::default();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(vm.execute(&mut s as &mut dyn PluginSession, &mut log, &rt))
        .expect_err("expected RuntimeError, got Ok")
}

pub(super) fn run_expect_runtime_err_with_limits(
    source: &str,
    session: MockPluginSession,
    limits: VmLimits,
) -> RuntimeError {
    let vm = make_vm_with_limits(source, limits);
    let mut log = PluginLog::new("test");
    let mut s = session;
    let rt = DslRuntime::default();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(vm.execute(&mut s as &mut dyn PluginSession, &mut log, &rt))
        .expect_err("expected RuntimeError, got Ok")
}

/// Run a hand-crafted CompiledScript and expect a RuntimeError.
/// Use this when the normal compiler cannot produce bytecode that triggers the variant.
pub(super) fn run_expect_runtime_err_on_script(
    script: CompiledScript,
    session: MockPluginSession,
    limits: VmLimits,
) -> RuntimeError {
    let vm = Vm::new(script, limits).expect("vm creation failed");
    let mut log = PluginLog::new("test");
    let mut s = session;
    let rt = DslRuntime::default();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(vm.execute(&mut s as &mut dyn PluginSession, &mut log, &rt))
        .expect_err("expected RuntimeError, got Ok")
}

pub(super) fn good_next() -> PluginRunningResult {
    PluginRunningResult::GoodNext
}

pub(super) fn err_response_body(status: u16, body: &str) -> PluginRunningResult {
    PluginRunningResult::ErrResponse {
        status,
        body: Some(body.to_string()),
    }
}

/// Run `source` and assert that the script sets request header `name` to `value` exactly once.
pub(super) fn assert_sets_request_header(source: &str, name: &'static str, value: &str) {
    let value = value.to_string();
    let mut session = MockPluginSession::new();
    session
        .expect_set_request_header()
        .withf(move |h, v| h == name && v == value)
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(source, session);
    assert_eq!(result, good_next());
}

// ---------------------------------------------------------------------------
// Smoke test
// ---------------------------------------------------------------------------

#[test]
fn test_smoke_compile_and_run() {
    let session = MockPluginSession::new();
    let result = run("", session);
    assert_eq!(result, good_next());
}
