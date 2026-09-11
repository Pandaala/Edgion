//! Tests for ctx.* builtins: get / set / remove and ctx.get string limits.
//!
//! Covers present and missing values, reading a pre-existing value in the
//! upstream_response_filter phase, and the ApiError path returned by fallible
//! session.set_ctx_var / remove_ctx_var.

use super::*;
use crate::EdgionHttpContext;

// ── Happy paths ──────────────────────────────────────────────────────────────

#[test]
fn test_ctx_set_then_get() {
    let mut session = MockPluginSession::new();
    session
        .expect_set_ctx_var()
        .withf(|k, v| k == "k" && v == "v")
        .times(1)
        .returning(|_, _| Ok(()));
    let mut ctx = EdgionHttpContext::new();
    ctx.set_ctx_var("k".into(), "v".into());
    session.expect_ctx().times(1).return_const(ctx);
    session.expect_get_ctx_var().never();
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-Ctx" && v == "v")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        r#"ctx.set("k", "v")
let value = ctx.get("k")
req.set_header("X-Ctx", value)"#,
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_ctx_set_then_remove_then_get() {
    let mut session = MockPluginSession::new();
    session
        .expect_set_ctx_var()
        .withf(|k, v| k == "k" && v == "v")
        .times(1)
        .returning(|_, _| Ok(()));
    session
        .expect_remove_ctx_var()
        .withf(|k| k == "k")
        .times(1)
        .returning(|_| Ok(()));
    session.expect_ctx().times(1).return_const(EdgionHttpContext::new());
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-Removed" && v == "true")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        r#"ctx.set("k", "v")
ctx.remove("k")
let value = ctx.get("k")
if value == nil { req.set_header("X-Removed", "true") }"#,
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_ctx_get_missing_returns_nil() {
    let mut session = MockPluginSession::new();
    session.expect_ctx().times(1).return_const(EdgionHttpContext::new());
    session
        .expect_set_request_header()
        .withf(|h, v| h == "X-NoVal" && v == "true")
        .times(1)
        .returning(|_, _| Ok(()));
    let result = run(
        r#"let value = ctx.get("missing")
if value == nil { req.set_header("X-NoVal", "true") }"#,
        session,
    );
    assert_eq!(result, good_next());
}

#[test]
fn test_ctx_get_value_exceeding_max_string_len_returns_exact_error() {
    let mut session = MockPluginSession::new();
    let mut ctx = EdgionHttpContext::new();
    ctx.set_ctx_var("k".into(), "four".into());
    session.expect_ctx().times(1).return_const(ctx);
    let limits = VmLimits {
        max_string_len: 3,
        ..Default::default()
    };

    let error = run_expect_runtime_err_with_limits(r#"ctx.get("k")"#, session, limits);

    assert!(matches!(error, RuntimeError::StringTooLong { len: 4, limit: 3 }));
}

// ── Cross-phase: ctx.get in upstream_response_filter ─────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn test_ctx_get_reads_pre_existing_value_in_upstream_response() {
    use crate::plugins::http::dsl::config::DslConfig;
    use crate::plugins::http::dsl::plugin::DslPlugin;
    let mut session = MockPluginSession::new();
    let mut ctx = EdgionHttpContext::new();
    ctx.set_ctx_var("k".into(), "pre_existing".into());
    session.expect_ctx().times(1).return_const(ctx);
    session
        .expect_set_response_header()
        .withf(|h, v| h == "X-Ctx-Out" && v == "pre_existing")
        .times(1)
        .returning(|_, _| Ok(()));

    let config = DslConfig {
        source: Some(
            r#"let value = ctx.get("k")
if value != nil { resp.set_header("X-Ctx-Out", value) }"#
                .to_string(),
        ),
        name: "test-ctx-resp".into(),
        ..Default::default()
    };
    let plugin = DslPlugin::from_config_response(&config).expect("from_config_response failed");
    let mut log = PluginLog::new("test");
    let mut s = session;
    let result = plugin.run_upstream_response_filter(&mut s as &mut dyn PluginSession, &mut log);
    assert_eq!(result, good_next());
}

// ── Error paths: session-side failure → RuntimeError::ApiError ───────────────

#[test]
fn test_ctx_set_session_error_yields_api_error() {
    let mut session = MockPluginSession::new();
    session
        .expect_set_ctx_var()
        .withf(|k, v| k == "x" && v == "y")
        .times(1)
        .returning(|_, _| Err(Box::new(std::io::Error::other("injected ctx.set failure"))));
    let err = run_expect_runtime_err(r#"ctx.set("x", "y")"#, session);
    assert!(matches!(
        err,
        RuntimeError::ApiError { ref function, .. } if function == "ctx.set"
    ));
}

#[test]
fn test_ctx_remove_session_error_yields_api_error() {
    let mut session = MockPluginSession::new();
    session
        .expect_remove_ctx_var()
        .withf(|k| k == "x")
        .times(1)
        .returning(|_| Err(Box::new(std::io::Error::other("injected ctx.remove failure"))));
    let err = run_expect_runtime_err(r#"ctx.remove("x")"#, session);
    assert!(matches!(
        err,
        RuntimeError::ApiError { ref function, .. } if function == "ctx.remove"
    ));
}
