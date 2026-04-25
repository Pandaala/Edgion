//! Tests for VM sandbox limits: step limit, loop limit, string length limit, error policy variants.

use super::*;

#[test]
fn test_step_limit_exceeded() {
    let session = MockPluginSession::new();
    let limits = VmLimits {
        max_steps: 5,
        ..Default::default()
    };
    let result = run_with_limits(
        "let a=1\nlet b=2\nlet c=3\nlet d=4\nlet e=5\nlet f=6\nlet g=7\nlet h=8\nlet k=9\nlet l=10",
        session,
        limits,
    );
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
}

#[test]
fn test_loop_limit_exceeded() {
    let session = MockPluginSession::new();
    let limits = VmLimits {
        max_loop_iterations: 3,
        ..Default::default()
    };
    let result = run_with_limits(r#"while true { }"#, session, limits);
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
}

#[test]
fn test_string_too_long() {
    let session = MockPluginSession::new();
    let limits = VmLimits {
        max_string_len: 5,
        ..Default::default()
    };
    let result = run_with_limits(r#"let s = "hello" + " world""#, session, limits);
    assert!(matches!(result, PluginRunningResult::ErrResponse { status: 500, .. }));
}

#[test]
fn test_step_limit_with_ignore() {
    use crate::core::gateway::plugins::http::dsl::lang::bytecode::CompiledScript;
    use crate::core::gateway::plugins::http::dsl::lang::validator::{compile_dsl_source, ValidationLimits};
    use crate::core::gateway::plugins::http::dsl::lang::vm::{execute_safe, DslErrorPolicy, Vm, VmLimits};

    let b64 = compile_dsl_source(
        "let a=1\nlet b=2\nlet c=3\nlet d=4\nlet e=5\nlet f=6",
        &ValidationLimits::default(),
    )
    .expect("compile failed");
    let script = CompiledScript::deserialize_base64(&b64).expect("deserialize failed");
    let vm = Vm::new(
        script,
        VmLimits {
            max_steps: 3,
            ..Default::default()
        },
    )
    .expect("vm failed");
    let session = MockPluginSession::new();
    let mut log = PluginLog::new("test");
    let mut s = session;
    let result = execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &DslErrorPolicy::Ignore);
    assert_eq!(result, good_next());
}

#[test]
fn test_loop_limit_with_deny_with() {
    use crate::core::gateway::plugins::http::dsl::lang::bytecode::CompiledScript;
    use crate::core::gateway::plugins::http::dsl::lang::validator::{compile_dsl_source, ValidationLimits};
    use crate::core::gateway::plugins::http::dsl::lang::vm::{execute_safe, DslErrorPolicy, Vm, VmLimits};

    let b64 = compile_dsl_source(r#"while true { }"#, &ValidationLimits::default()).expect("compile failed");
    let script = CompiledScript::deserialize_base64(&b64).expect("deserialize failed");
    let vm = Vm::new(
        script,
        VmLimits {
            max_loop_iterations: 2,
            ..Default::default()
        },
    )
    .expect("vm failed");
    let session = MockPluginSession::new();
    let mut log = PluginLog::new("test");
    let mut s = session;
    let policy = DslErrorPolicy::DenyWith {
        status: 429,
        body: Some("slow".to_string()),
    };
    let result = execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &policy);
    assert_eq!(result, err_response_body(429, "slow"));
}
