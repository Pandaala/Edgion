//! Tests for DslErrorPolicy variants: Ignore, Deny, DenyWith.

use super::*;
use crate::core::gateway::plugins::http::dsl::lang::bytecode::CompiledScript;
use crate::core::gateway::plugins::http::dsl::lang::validator::{compile_dsl_source, ValidationLimits};
use crate::core::gateway::plugins::http::dsl::lang::vm::{execute_safe, DslErrorPolicy, Vm, VmLimits};

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
    let session = MockPluginSession::new();
    let mut log = PluginLog::new("test");
    let mut s = session;
    let result = execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &DslErrorPolicy::Ignore);
    assert_eq!(result, good_next());
}

#[test]
fn test_policy_deny() {
    let vm = limited_vm();
    let session = MockPluginSession::new();
    let mut log = PluginLog::new("test");
    let mut s = session;
    let result = execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &DslErrorPolicy::Deny);
    assert_eq!(result, err_response_body(500, "DSL execution error"));
}

#[test]
fn test_policy_deny_with() {
    let vm = limited_vm();
    let session = MockPluginSession::new();
    let mut log = PluginLog::new("test");
    let mut s = session;
    let policy = DslErrorPolicy::DenyWith {
        status: 418,
        body: Some("teapot".to_string()),
    };
    let result = execute_safe(&vm, &mut s as &mut dyn PluginSession, &mut log, &policy);
    assert_eq!(result, err_response_body(418, "teapot"));
}
