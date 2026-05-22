//! Tests for DslErrorPolicy variants: Ignore, Deny, DenyWith.

use super::*;

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
