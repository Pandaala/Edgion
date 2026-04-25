//! Unit tests for EdgionDSL VM execution semantics.
//!
//! Uses MockPluginSession to validate behaviour without real HTTP connections.
//! MockPluginSession is not Send — all tests use #[test], not #[tokio::test].

pub(super) use super::lang::bytecode::CompiledScript;
pub(super) use super::lang::validator::{compile_dsl_source, ValidationLimits};
pub(super) use super::lang::vm::{execute_safe, DslErrorPolicy, Vm, VmLimits};
pub(super) use crate::core::gateway::plugins::runtime::traits::session::{MockPluginSession, PluginSession};
pub(super) use crate::core::gateway::plugins::runtime::PluginLog;
pub(super) use crate::types::filters::PluginRunningResult;

mod compile_errors;
mod control_flow;
mod error_policy;
mod grpc_error_tests;
mod p3_utility_tests;
mod range_and_string_ops;
mod req_metadata_tests;
mod request_builtins;
mod request_mutation;
mod resp_p2_tests;
mod resp_set_status_tests;
mod response_builtins;
mod sandbox;
mod terminate_tests;
mod utility_functions;

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
    execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &DslErrorPolicy::Deny)
}

pub(super) fn run_with_limits(source: &str, session: MockPluginSession, limits: VmLimits) -> PluginRunningResult {
    let vm = make_vm_with_limits(source, limits);
    let mut log = PluginLog::new("test");
    let mut s = session;
    execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &DslErrorPolicy::Deny)
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

// ---------------------------------------------------------------------------
// Smoke test
// ---------------------------------------------------------------------------

#[test]
fn test_smoke_compile_and_run() {
    let session = MockPluginSession::new();
    let result = run("", session);
    assert_eq!(result, good_next());
}
