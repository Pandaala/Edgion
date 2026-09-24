//! Tests for the final-decision PluginLog entry emitted by `execute_safe`.
//!
//! The entry must reflect the actual decision using the project-wide PluginLog
//! convention (`OK` / `Deny <detail>`), must not carry the legacy `; ` suffix,
//! and must not emit the stale `dsl match` token regardless of outcome.
//! Error-policy paths emit only the `err: ` entry, never a decision token.

use super::*;
use expect_test::expect;

#[test]
fn test_decision_log_next_emits_ok() {
    let vm = make_vm("return next()");
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    expect!["OK"].assert_eq(&log.render_for_test());
}

#[test]
fn test_decision_log_implicit_return_emits_ok() {
    let vm = make_vm("let x = 1");
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    expect!["OK"].assert_eq(&log.render_for_test());
}

#[test]
fn test_decision_log_deny_emits_status() {
    let vm = make_vm(r#"return deny(403, "blocked")"#);
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, err_response_body(403, "blocked"));
    expect!["Deny 403"].assert_eq(&log.render_for_test());
}

#[test]
fn test_decision_log_terminate_emits_deny_once() {
    let vm = make_vm("return terminate()");
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 502,
            body: None
        }
    );
    expect!["Deny 502"].assert_eq(&log.render_for_test());
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
    expect!["Deny grpc 7"].assert_eq(&log.render_for_test());
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

    // The exact-string snapshot guards the precise log format.
    // The three negative `!contains` probes below carry invariant-explanation messages
    // a reviewer reads in failure output — they protect against a regression that
    // re-introduces a decision token on the error path, where the snapshot alone
    // would only show "string differs" without the "why".
    expect!["err: step limit exceeded"].assert_eq(&log.render_for_test());

    // Invariants the snapshot alone cannot articulate to a reviewer:
    assert!(!log.contains("OK"), "error path must not emit 'OK' decision token");
    assert!(
        !log.contains("Deny"),
        "error path must not emit a 'Deny' decision token"
    );
    assert!(!log.contains("dsl match"), "stale 'dsl match' entry must not appear");
}
