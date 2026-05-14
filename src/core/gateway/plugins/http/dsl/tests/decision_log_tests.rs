//! Tests for the final-decision PluginLog entry emitted by `execute_safe`.
//!
//! The entry must reflect the actual decision using the project-wide PluginLog
//! convention (`OK; ` / `Deny <detail>; `), must carry the `; ` suffix, and must
//! not emit the stale `dsl match` token regardless of outcome. Error-policy
//! paths emit only the `err: ` entry, never a decision token.

use super::*;

#[test]
fn test_decision_log_next_emits_ok() {
    let vm = make_vm("return next()");
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    assert!(log.contains("OK; "), "expected 'OK; ' in log");
    assert!(!log.contains("dsl match"), "stale 'dsl match' entry");
}

#[test]
fn test_decision_log_implicit_return_emits_ok() {
    let vm = make_vm("let x = 1");
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    assert!(log.contains("OK; "), "expected 'OK; ' in log for implicit return");
}

#[test]
fn test_decision_log_deny_emits_status() {
    let vm = make_vm(r#"return deny(403, "blocked")"#);
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, err_response_body(403, "blocked"));
    assert!(log.contains("Deny 403; "), "expected 'Deny 403; ' in log");
    assert!(!log.contains("dsl match"), "stale 'dsl match' entry");
}

#[test]
fn test_decision_log_terminate_emits_deny_once() {
    let vm = make_vm("return terminate()");
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    assert!(log.contains("Deny terminate; "), "expected 'Deny terminate; ' in log");
    assert!(!log.contains("dsl match"), "stale 'dsl match' entry");
    assert!(!log.contains("Deny dsl terminate"), "duplicate in-VM terminate entry");
}

#[test]
fn test_decision_log_grpc_error_emits_deny() {
    let vm = make_vm(r#"return grpc_error(7, "permission denied")"#);
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(
        result,
        PluginRunningResult::ErrGrpcError {
            grpc_status: 7,
            message: Some("permission denied".to_string()),
        }
    );
    assert!(log.contains("Deny grpc 7; "), "expected 'Deny grpc 7; ' in log");
    assert!(!log.contains("dsl match"), "stale 'dsl match' entry");
}

#[test]
fn test_decision_log_runtime_error_emits_only_err_entry() {
    // max_steps: 3 forces StepLimitExceeded before any explicit return.
    let vm = make_vm_with_limits(
        "let a = 1\nlet b = 2\nlet c = 3\nlet d = 4",
        VmLimits {
            max_steps: 3,
            ..Default::default()
        },
    );
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, err_response_body(500, "DSL execution error"));
    assert!(log.contains("err: "), "expected 'err: ' entry in log");
    // The error path must not also push a decision token.
    assert!(!log.contains("OK"), "error path must not emit 'OK'");
    assert!(
        !log.contains("Deny"),
        "error path must not emit a 'Deny' decision token"
    );
    assert!(!log.contains("dsl match"), "stale 'dsl match' entry");
}
