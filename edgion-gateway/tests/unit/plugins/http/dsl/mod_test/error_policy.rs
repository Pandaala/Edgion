//! Tests for DslErrorPolicy variants, panic isolation, and fixed BodyAccess handling.

use super::*;
use crate::plugins::runtime::DyeOutcome;
use crate::request_body::BodyReadError;
use crate::request_body::BodyStorage;
use crate::EdgionHttpContext;
use pingora_core::protocols::http::RequestBodyBuffer;

#[tokio::test]
async fn body_text_overflow_obeys_string_error_policy_while_invalid_range_fails_closed() {
    for policy in [
        DslErrorPolicy::Ignore,
        DslErrorPolicy::Deny,
        DslErrorPolicy::DenyWith {
            status: 418,
            body: Some("teapot".to_string()),
        },
    ] {
        for source in ["req.body.read(0, 1)", "req.body.read(0, 3)", "req.body.read(2, 1)"] {
            let vm = make_vm_with_limits(
                source,
                VmLimits {
                    max_string_len: 2,
                    ..VmLimits::default()
                },
            );
            let (mut storage, view) = BodyStorage::new(0, 1);
            storage.write(&Bytes::from_static(&[0xff])).await.unwrap();
            storage.finish().await.unwrap();
            let mut ctx = EdgionHttpContext::new();
            ctx.request_body_config.max_body_size_bytes = 1;
            ctx.request_body.install(view);
            let mut session = MockPluginSession::new();
            session.expect_declares_body().return_const(Some(true));
            session.expect_request_body_available_len().return_const(Ok(1));
            session.expect_ctx().return_const(ctx);
            session.expect_record_outcome().times(1).return_const(());
            let mut log = PluginLog::new("test");
            let result = execute_safe(&vm, &mut session, &mut log, &policy, &DslRuntime::default()).await;

            if source == "req.body.read(2, 1)" {
                assert_eq!(result, err_response_body(500, "Request body processing failed"));
                assert_eq!(log.render_for_test(), "err: request body access error");
            } else {
                let expected = match &policy {
                    DslErrorPolicy::Ignore => good_next(),
                    DslErrorPolicy::Deny => err_response_body(500, "DSL execution error"),
                    DslErrorPolicy::DenyWith { .. } => err_response_body(418, "teapot"),
                };
                assert_eq!(result, expected);
                assert_eq!(log.render_for_test(), "err: string too long");
            }
        }
    }
}

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

#[test]
fn test_policy_deny_with_informational_status_fails_safe() {
    let vm = limited_vm();
    let policy = DslErrorPolicy::DenyWith {
        status: 103,
        body: Some("invalid".to_string()),
    };
    let result = run_vm_policy(&vm, MockPluginSession::new(), &policy);
    assert_eq!(result, err_response_body(500, "invalid"));
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

fn run_vm_policy_with_session(
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

#[test]
fn test_policy_ignore_records_fail_open() {
    let vm = limited_vm();
    let mut session = MockPluginSession::new();
    session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailOpen) && value == "script-error")
        .times(1)
        .return_const(());
    let (result, _) = run_vm_policy_with_session(&vm, session, &DslErrorPolicy::Ignore);
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
    let (result, _) = run_vm_policy_with_session(&vm, session, &DslErrorPolicy::Deny);
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
    let (result, _) = run_vm_policy_with_session(&vm, session, &policy);
    assert_eq!(result, err_response_body(418, "teapot"));
}

#[test]
fn test_policy_ignore_catches_vm_panic_and_records_fail_open() {
    let vm = make_vm(r#"let value = req.query("X-Panic")"#);
    let mut session = MockPluginSession::new();
    session
        .expect_get_query_param()
        .withf(|name| name == "X-Panic")
        .times(1)
        .returning(|_| panic!("injected query read panic"));
    session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailOpen) && value == "script-error")
        .times(1)
        .return_const(());

    let (result, log) = run_vm_policy_with_session(&vm, session, &DslErrorPolicy::Ignore);

    assert_eq!(result, good_next());
    assert_eq!(log.render_for_test(), "err: vm panic");
    assert!(
        !log.contains("OK") && !log.contains("Deny"),
        "panic path must not emit a normal decision entry"
    );
}

#[test]
fn test_body_access_bypasses_ignore_and_records_fixed_fail_close() {
    let vm = make_vm("let b = req.body");
    let mut session = MockPluginSession::new();
    session.expect_declares_body().return_const(Some(true));
    session
        .expect_request_body_available_len()
        .times(1)
        .returning(|_| Err(BodyReadError::Unimplemented));
    session
        .expect_record_outcome()
        .withf(|outcome, value| matches!(outcome, DyeOutcome::FailClose) && value == "request-body-access-error")
        .times(1)
        .return_const(());

    let (result, log) = run_vm_policy_with_session(&vm, session, &DslErrorPolicy::Ignore);

    assert_eq!(result, err_response_body(500, "Request body processing failed"));
    assert_eq!(log.render_for_test(), "err: request body access error");
    assert!(
        !log.contains("OK") && !log.contains("Deny"),
        "BodyAccess path must not emit a normal decision entry"
    );
}
