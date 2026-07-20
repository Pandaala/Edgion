//! Tests for DslErrorPolicy variants: Ignore, Deny, DenyWith.

use super::*;
use crate::plugins::runtime::DyeOutcome;

fn limited_vm() -> Vm {
    let b64 = compile_dsl_source(
        "let a=1\nlet b=2\nlet c=3\nlet d=4\nlet e=5\nlet f=6",
        &ValidationLimits::default(),
    )
    .expect("compile failed");
    let script = CompiledScript::deserialize_base64(&b64).expect("deserialize failed");
    Vm::new(
        script,
        VmLimits {
            max_steps: 3,
            ..Default::default()
        },
    )
    .expect("vm failed")
}

#[test]
fn test_policy_ignore() {
    let vm = limited_vm();
    let result = run_vm_policy(&vm, MockPluginSession::new(), &DslErrorPolicy::Ignore);
    assert_eq!(result, good_next());
}

#[test]
fn test_policy_deny() {
    let vm = limited_vm();
    let result = run_vm_policy(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, err_response_body(500, "DSL execution error"));
}

#[test]
fn test_policy_deny_with() {
    let vm = limited_vm();
    let policy = DslErrorPolicy::DenyWith {
        status: 418,
        body: Some("teapot".to_string()),
    };
    let result = run_vm_policy(&vm, MockPluginSession::new(), &policy);
    assert_eq!(result, err_response_body(418, "teapot"));
}

// ── Task 9: dye-suite record_outcome wiring ─────────────────────────────
//
// `apply_error_policy` (the single choke point for every DSL error-policy
// decision) calls `record_outcome` before returning its `PluginRunningResult`
// value. Unlike rate_limit_redis/forward_auth/ldap_auth, the DSL plugin
// never self-writes a response — `ErrResponse` is a value the runtime writes
// AFTER this call returns — so "record before write" is guaranteed by
// `apply_error_policy`'s own control flow (record, then return) rather than
// needing a mockall `Sequence` against a self-write call.
//
// These three tests call `execute_safe` directly instead of going through
// `run_vm_policy`, because that helper installs a permissive catch-all
// `expect_record_outcome` (see `tests/mod.rs`) for the ~20 other callers that
// don't care about dye behavior; a narrow `.times(1)` expectation added here
// after that catch-all would never be reached (mockall matches newest-first).

fn run_vm_policy_with_session(vm: &Vm, mut session: MockPluginSession, policy: &DslErrorPolicy) -> PluginRunningResult {
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

#[test]
fn test_policy_ignore_records_fail_open() {
    let vm = limited_vm();
    let mut session = MockPluginSession::new();
    session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailOpen) && value == "script-error")
        .times(1)
        .return_const(());
    let result = run_vm_policy_with_session(&vm, session, &DslErrorPolicy::Ignore);
    assert_eq!(result, good_next());
}

#[test]
fn test_policy_deny_records_fail_close() {
    let vm = limited_vm();
    let mut session = MockPluginSession::new();
    session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "script-error")
        .times(1)
        .return_const(());
    let result = run_vm_policy_with_session(&vm, session, &DslErrorPolicy::Deny);
    assert_eq!(result, err_response_body(500, "DSL execution error"));
}

#[test]
fn test_policy_deny_with_records_fail_close() {
    let vm = limited_vm();
    let policy = DslErrorPolicy::DenyWith {
        status: 418,
        body: Some("teapot".to_string()),
    };
    let mut session = MockPluginSession::new();
    session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "script-error")
        .times(1)
        .return_const(());
    let result = run_vm_policy_with_session(&vm, session, &policy);
    assert_eq!(result, err_response_body(418, "teapot"));
}
