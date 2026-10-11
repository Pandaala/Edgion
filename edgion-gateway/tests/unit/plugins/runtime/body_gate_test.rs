//! Unit tests for `body_gate.rs` (see `super`).
use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_condition_input_mocks;
use crate::request_body::BodyReadError;
use crate::EdgionHttpContext;
use bytes::Bytes;
use edgion_resources::common::KeyGet;
use edgion_resources::resources::edgion_plugins::conditions::{
    Condition, ConditionSet, KeyMatchCondition, PluginConditions,
};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A mock session that carries a default ctx (so cap resolution has what
/// it needs). The default test filter requires full materialization, so
/// tests layer a `read_request_body` expectation on top.
fn base_session() -> MockPluginSession {
    let mut session = MockPluginSession::new();
    session.expect_ctx().return_const(EdgionHttpContext::new());
    session.expect_record_error().returning(|_| ());
    session
}

struct CountingFilter {
    name: String,
    run_count: AtomicUsize,
}

impl CountingFilter {
    fn new() -> Self {
        Self {
            name: "Counting".to_string(),
            run_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl RequestFilter for CountingFilter {
    fn name(&self) -> &str {
        &self.name
    }
    async fn run_request(&self, _s: &mut dyn PluginSession, _l: &mut PluginLog) -> PluginRunningResult {
        self.run_count.fetch_add(1, Ordering::SeqCst);
        PluginRunningResult::GoodNext
    }
}

#[test]
fn runtime_policy_caches_resolved_fields_at_construction() {
    let requirement = BodyRequirement {
        max_body_size: Some("64M".to_string()),
        conditions: Some(PluginConditions {
            skip: None,
            run: Some(ConditionSet::all_of(vec![Condition::KeyMatch(KeyMatchCondition {
                key: KeyGet::Method,
                core: edgion_resources::resources::edgion_plugins::conditions::MatchCore {
                    values: Some(vec!["POST".to_string()]),
                    ..Default::default()
                },
                negate: false,
                descriptor: Box::from(""),
            })])),
        }),
        on_read_failure: BodyReadFailureMode::FailOpen,
    };

    let policy = BodyGatePolicy::new(requirement);

    assert_eq!(policy.logical_limit_bytes, Some(64 * 1024 * 1024));
    assert_eq!(policy.on_read_failure, BodyReadFailureMode::FailOpen);
    assert!(policy.conditions.is_some());
}

#[test]
fn runtime_policy_normalizes_empty_conditions() {
    let policy = BodyGatePolicy::new(BodyRequirement {
        conditions: Some(PluginConditions::default()),
        ..BodyRequirement::fail_close()
    });

    assert!(policy.conditions.is_none());
}

// The default session returns Unimplemented for read_request_body.
#[tokio::test]
async fn fail_close_returns_500_and_skips_inner() {
    let inner = CountingFilter::new();
    let gate = BodyGatedRequestFilter::new(Box::new(inner), BodyRequirement::fail_close());

    let mut session = base_session();
    session
        .expect_read_request_body()
        .returning(|_| Err(BodyReadError::Unimplemented));

    let mut log = PluginLog::new("test");
    let result = gate.run_request(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None
        }
    );
}

#[tokio::test]
async fn too_large_maps_to_413() {
    let inner = CountingFilter::new();
    let gate = BodyGatedRequestFilter::new(Box::new(inner), BodyRequirement::fail_close());

    let mut session = base_session();
    session
        .expect_read_request_body()
        .returning(|_| Err(BodyReadError::TooLarge { limit: 1024 }));

    let mut log = PluginLog::new("test");
    let result = gate.run_request(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 413,
            body: None
        }
    );
}

#[tokio::test]
async fn fail_open_skips_plugin() {
    let inner = CountingFilter::new();
    let req = BodyRequirement {
        on_read_failure: BodyReadFailureMode::FailOpen,
        ..Default::default()
    };
    let gate = BodyGatedRequestFilter::new(Box::new(inner), req);

    let mut session = base_session();
    session
        .expect_read_request_body()
        .returning(|_| Err(BodyReadError::Unimplemented));

    let mut log = PluginLog::new("test");
    let result = gate.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::Nothing);
}

#[tokio::test]
async fn gateway_buffer_limit_cannot_fail_open() {
    let inner = CountingFilter::new();
    let req = BodyRequirement {
        on_read_failure: BodyReadFailureMode::FailOpen,
        ..Default::default()
    };
    let gate = BodyGatedRequestFilter::new(Box::new(inner), req);

    let mut session = base_session();
    session
        .expect_read_request_body()
        .returning(|_| Err(BodyReadError::BufferLimitExceeded { limit: 1024 }));

    let mut log = PluginLog::new("test");
    assert_eq!(
        gate.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse {
            status: 413,
            body: None
        }
    );
}

#[tokio::test]
async fn ok_buffer_runs_inner() {
    let inner = CountingFilter::new();
    let gate = BodyGatedRequestFilter::new(Box::new(inner), BodyRequirement::fail_close());

    let mut session = base_session();
    session
        .expect_read_request_body()
        .returning(|_| Ok(Bytes::from_static(b"{}")));

    let mut log = PluginLog::new("test");
    let result = gate.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
}

#[tokio::test]
async fn secondary_skip_runs_inner_without_buffering() {
    let inner = CountingFilter::new();
    // The first leaf matches and the second fails. A multi-leaf body gate
    // must remain log-less while skipping body capture.
    let req = BodyRequirement {
        conditions: Some(PluginConditions {
            skip: None,
            run: Some(ConditionSet::all_of(vec![
                Condition::KeyMatch(KeyMatchCondition {
                    key: KeyGet::Method,
                    core: edgion_resources::resources::edgion_plugins::conditions::MatchCore {
                        values: Some(vec!["GET".to_string()]),
                        ..Default::default()
                    },
                    negate: false,
                    descriptor: Box::from(""),
                }),
                Condition::KeyMatch(KeyMatchCondition {
                    key: KeyGet::Header {
                        name: "X-Body-Gate".to_string(),
                    },
                    core: edgion_resources::resources::edgion_plugins::conditions::MatchCore {
                        values: Some(vec!["yes".to_string()]),
                        ..Default::default()
                    },
                    negate: false,
                    descriptor: Box::from(""),
                }),
            ])),
        }),
        ..Default::default()
    };
    let gate = BodyGatedRequestFilter::new(Box::new(inner), req);

    let mut session = MockPluginSession::new();
    setup_condition_input_mocks(&mut session, &[(KeyGet::Method, "GET")]);
    // read_request_body must NOT be called; no expectation set.

    let mut log = PluginLog::new("test");
    let result = gate.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
    assert!(!log.contains("run.allOf[0]"));
}

#[tokio::test]
async fn secondary_regex_condition_is_compiled_and_evaluated() {
    // F3 regression: a REGEX secondary condition must be compiled to evaluate.
    // Run-condition `^POST$` on the method; request is POST → the compiled
    // regex MATCHES → body IS required → read is attempted → fail-close 500.
    // If the regex were left uncompiled (the old bug), the match would
    // silently return false → skip → inner runs → GoodNext. Asserting the 500
    // proves the compiled regex actually evaluated.
    let mut conds = PluginConditions {
        skip: None,
        run: Some(ConditionSet::all_of(vec![Condition::KeyMatch(KeyMatchCondition {
            key: KeyGet::Method,
            core: edgion_resources::resources::edgion_plugins::conditions::MatchCore {
                regex: Some(vec!["^POST$".to_string()]),
                ..Default::default()
            },
            negate: false,
            descriptor: Box::from(""),
        })])),
    };
    conds.compile().expect("regex compiles");
    let req = BodyRequirement {
        conditions: Some(conds),
        ..BodyRequirement::fail_close()
    };
    let gate = BodyGatedRequestFilter::new(Box::new(CountingFilter::new()), req);

    let mut session = base_session();
    setup_condition_input_mocks(&mut session, &[(KeyGet::Method, "POST")]);
    session
        .expect_read_request_body()
        .returning(|_| Err(BodyReadError::Unimplemented));

    let mut log = PluginLog::new("test");
    let result = gate.run_request(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None
        }
    );
}

#[tokio::test]
async fn per_plugin_max_body_size_rejects_oversized_body() {
    // The per-plugin maxBodySize is a LIMIT (not a buffer knob): a body larger
    // than the plugin's own limit → TooLarge → 413. The read returns a
    // 100-byte body; the plugin limit is 10 bytes.
    let req = BodyRequirement {
        max_body_size: Some("10".to_string()),
        ..BodyRequirement::fail_close()
    };
    let gate = BodyGatedRequestFilter::new(Box::new(CountingFilter::new()), req);

    let mut session = base_session();
    session
        .expect_read_request_body()
        .returning(|_| Ok(Bytes::from_static(&[0u8; 100])));

    let mut log = PluginLog::new("test");
    let result = gate.run_request(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 413,
            body: None
        }
    );
}

#[tokio::test]
async fn per_plugin_max_body_size_allows_body_within_limit() {
    let req = BodyRequirement {
        max_body_size: Some("1k".to_string()),
        ..BodyRequirement::fail_close()
    };
    let gate = BodyGatedRequestFilter::new(Box::new(CountingFilter::new()), req);

    let mut session = base_session();
    session
        .expect_read_request_body()
        .returning(|_| Ok(Bytes::from_static(b"small")));

    let mut log = PluginLog::new("test");
    let result = gate.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
}

#[tokio::test]
async fn per_plugin_max_body_size_over_limit_fail_open_skips() {
    // Over the per-plugin limit + fail-open → skip (Nothing), not 413.
    let req = BodyRequirement {
        max_body_size: Some("10".to_string()),
        on_read_failure: BodyReadFailureMode::FailOpen,
        ..Default::default()
    };
    let gate = BodyGatedRequestFilter::new(Box::new(CountingFilter::new()), req);
    let mut session = base_session();
    session
        .expect_read_request_body()
        .returning(|_| Ok(Bytes::from_static(&[0u8; 100])));

    let mut log = PluginLog::new("test");
    let result = gate.run_request(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::Nothing);
}

#[tokio::test]
async fn per_plugin_max_body_size_boundary() {
    // body.len() == limit passes; == limit + 1 rejects (guards the `>` check).
    async fn run_with_body(body: &'static [u8]) -> PluginRunningResult {
        let req = BodyRequirement {
            max_body_size: Some("10".to_string()),
            ..BodyRequirement::fail_close()
        };
        let gate = BodyGatedRequestFilter::new(Box::new(CountingFilter::new()), req);
        let mut session = base_session();
        session
            .expect_read_request_body()
            .returning(move |_| Ok(Bytes::from_static(body)));
        let mut log = PluginLog::new("test");
        gate.run_request(&mut session, &mut log).await
    }
    assert_eq!(run_with_body(&[0u8; 10]).await, PluginRunningResult::GoodNext);
    assert_eq!(
        run_with_body(&[0u8; 11]).await,
        PluginRunningResult::ErrResponse {
            status: 413,
            body: None
        }
    );
}

// ===== Response-stage gate (mirrors the request-stage cases) =====

struct CountingResponse {
    name: String,
}

#[async_trait]
impl UpstreamResponse for CountingResponse {
    fn name(&self) -> &str {
        &self.name
    }
    async fn run_upstream_response(&self, _s: &mut dyn PluginSession, _l: &mut PluginLog) -> PluginRunningResult {
        PluginRunningResult::GoodNext
    }
}

fn response_gate() -> BodyGatedUpstreamResponse {
    BodyGatedUpstreamResponse::new(
        Box::new(CountingResponse {
            name: "CountingResp".to_string(),
        }),
        BodyRequirement::fail_close(),
    )
}

#[tokio::test]
async fn response_gate_fail_close_returns_500() {
    let gate = response_gate();
    let mut session = base_session();
    session
        .expect_read_response_body()
        .returning(|_| Err(BodyReadError::Unimplemented));

    let mut log = PluginLog::new("test");
    let result = gate.run_upstream_response(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None
        }
    );
}

#[tokio::test]
async fn response_gate_too_large_returns_413() {
    let gate = response_gate();
    let mut session = base_session();
    session
        .expect_read_response_body()
        .returning(|_| Err(BodyReadError::TooLarge { limit: 1024 }));

    let mut log = PluginLog::new("test");
    let result = gate.run_upstream_response(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 413,
            body: None
        }
    );
}

#[tokio::test]
async fn response_gate_ok_runs_inner() {
    let gate = response_gate();
    let mut session = base_session();
    session
        .expect_read_response_body()
        .returning(|_| Ok(Bytes::from_static(b"{}")));

    let mut log = PluginLog::new("test");
    let result = gate.run_upstream_response(&mut session, &mut log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
}

#[tokio::test]
async fn response_gate_per_plugin_limit_rejects_oversized() {
    // Response gate must apply `over_plugin_limit` to an Ok body (not just
    // map a pre-made TooLarge) — mirrors the request-gate per-plugin case.
    let gate = BodyGatedUpstreamResponse::new(
        Box::new(CountingResponse {
            name: "CountingResp".to_string(),
        }),
        BodyRequirement {
            max_body_size: Some("10".to_string()),
            ..BodyRequirement::fail_close()
        },
    );
    let mut session = base_session();
    session
        .expect_read_response_body()
        .returning(|_| Ok(Bytes::from_static(&[0u8; 100])));

    let mut log = PluginLog::new("test");
    let result = gate.run_upstream_response(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 413,
            body: None
        }
    );
}

#[tokio::test]
async fn timeout_maps_to_408() {
    let gate = BodyGatedRequestFilter::new(Box::new(CountingFilter::new()), BodyRequirement::fail_close());
    let mut session = base_session();
    session
        .expect_read_request_body()
        .returning(|_| Err(BodyReadError::Timeout));

    let mut log = PluginLog::new("test");
    let result = gate.run_request(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 408,
            body: None
        }
    );
}

#[tokio::test]
async fn refusal_is_fail_close_even_for_a_fail_open_plugin() {
    // R5. Without this, `Upgrade: websocket` or a forged
    // `content-type: application/grpc` on a POST would SKIP a fail-open body
    // consumer entirely — D1 with extra steps.
    let req = BodyRequirement {
        on_read_failure: BodyReadFailureMode::FailOpen,
        ..Default::default()
    };
    let gate = BodyGatedRequestFilter::new(Box::new(CountingFilter::new()), req);

    let mut session = base_session();
    session
        .expect_read_request_body()
        .returning(|_| Err(BodyReadError::NotBuffered));

    let mut log = PluginLog::new("test");
    let result = gate.run_request(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None
        },
        "a refusal must never be softened into a skip"
    );
}

#[tokio::test]
async fn response_gate_refusal_is_fail_close_even_for_a_fail_open_plugin() {
    let gate = BodyGatedUpstreamResponse::new(
        Box::new(CountingResponse {
            name: "CountingResp".to_string(),
        }),
        BodyRequirement {
            on_read_failure: BodyReadFailureMode::FailOpen,
            ..Default::default()
        },
    );

    let mut session = base_session();
    session
        .expect_read_response_body()
        .returning(|_| Err(BodyReadError::NotBuffered));

    let mut log = PluginLog::new("test");
    let result = gate.run_upstream_response(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None
        }
    );
}

#[tokio::test]
async fn genuine_read_failures_still_honor_fail_open() {
    // R6: the knob keeps governing real read failures. Uses `Timeout` rather
    // than `Unimplemented` (already covered by `fail_open_skips_plugin`) so
    // this test exercises a distinct variant instead of duplicating it.
    let req = BodyRequirement {
        on_read_failure: BodyReadFailureMode::FailOpen,
        ..Default::default()
    };
    let gate = BodyGatedRequestFilter::new(Box::new(CountingFilter::new()), req);

    let mut session = base_session();
    session
        .expect_read_request_body()
        .returning(|_| Err(BodyReadError::Timeout));

    let mut log = PluginLog::new("test");
    assert_eq!(
        gate.run_request(&mut session, &mut log).await,
        PluginRunningResult::Nothing
    );
}

// ===== Pin the BodyReadError -> EdgionErrorCode mapping =====
//
// `base_session()`'s `expect_record_error().returning(|_| ())` accepts any
// status, so it cannot catch a swapped arm in `body_read_failure_result`.
// These tests build a session mock local to each case with a `.withf(...)`
// predicate on the exact expected `EdgionErrorCode`, so a swapped mapping fails
// the test instead of silently passing.

/// A session mock like `base_session()`, but with a strict `record_error`
/// expectation pinned to `expected` (called exactly once) instead of the
/// permissive any-status stub.
fn session_expecting_error(expected: EdgionErrorCode) -> MockPluginSession {
    let mut session = MockPluginSession::new();
    session.expect_ctx().return_const(EdgionHttpContext::new());
    session
        .expect_record_error()
        .withf(move |status| *status == expected)
        .times(1)
        .returning(|_| ());
    session
}

#[tokio::test]
async fn too_large_records_body_read_too_large_status() {
    let gate = BodyGatedRequestFilter::new(Box::new(CountingFilter::new()), BodyRequirement::fail_close());
    let mut session = session_expecting_error(EdgionErrorCode::BodyReadTooLarge);
    session
        .expect_read_request_body()
        .returning(|_| Err(BodyReadError::TooLarge { limit: 1024 }));

    let mut log = PluginLog::new("test");
    gate.run_request(&mut session, &mut log).await;
}

#[tokio::test]
async fn timeout_records_body_read_timeout_status() {
    let gate = BodyGatedRequestFilter::new(Box::new(CountingFilter::new()), BodyRequirement::fail_close());
    let mut session = session_expecting_error(EdgionErrorCode::BodyReadTimeout);
    session
        .expect_read_request_body()
        .returning(|_| Err(BodyReadError::Timeout));

    let mut log = PluginLog::new("test");
    gate.run_request(&mut session, &mut log).await;
}

#[tokio::test]
async fn storage_failure_records_status_and_cannot_fail_open() {
    // A storage failure means consumed bytes were never stored: no
    // forwarding copy exists, so even a fail-open plugin must reject —
    // and the access log must carry the infra-fault status, not the
    // client-abort one.
    let req = BodyRequirement {
        on_read_failure: BodyReadFailureMode::FailOpen,
        ..Default::default()
    };
    let gate = BodyGatedRequestFilter::new(Box::new(CountingFilter::new()), req);
    let mut session = session_expecting_error(EdgionErrorCode::BodyCaptureStorageFailed);
    session
        .expect_read_request_body()
        .returning(|_| Err(BodyReadError::StorageFailure));

    let mut log = PluginLog::new("test");
    assert_eq!(
        gate.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None
        }
    );
}

#[tokio::test]
async fn incomplete_records_body_read_aborted_status() {
    let gate = BodyGatedRequestFilter::new(Box::new(CountingFilter::new()), BodyRequirement::fail_close());
    let mut session = session_expecting_error(EdgionErrorCode::BodyReadAborted);
    session
        .expect_read_request_body()
        .returning(|_| Err(BodyReadError::Incomplete));

    let mut log = PluginLog::new("test");
    gate.run_request(&mut session, &mut log).await;
}

#[tokio::test]
async fn unimplemented_records_body_read_unimplemented_status() {
    let gate = BodyGatedRequestFilter::new(Box::new(CountingFilter::new()), BodyRequirement::fail_close());
    let mut session = session_expecting_error(EdgionErrorCode::BodyReadUnimplemented);
    session
        .expect_read_request_body()
        .returning(|_| Err(BodyReadError::Unimplemented));

    let mut log = PluginLog::new("test");
    gate.run_request(&mut session, &mut log).await;
}

#[tokio::test]
async fn not_buffered_never_records_error_diagnostic() {
    // The read adapter already recorded the specific refusal reason on ctx;
    // the gate deliberately does not double-count it via `record_error`.
    let gate = BodyGatedRequestFilter::new(Box::new(CountingFilter::new()), BodyRequirement::fail_close());

    let mut session = MockPluginSession::new();
    session.expect_ctx().return_const(EdgionHttpContext::new());
    session.expect_record_error().times(0);
    session
        .expect_read_request_body()
        .returning(|_| Err(BodyReadError::NotBuffered));

    let mut log = PluginLog::new("test");
    let result = gate.run_request(&mut session, &mut log).await;
    assert_eq!(
        result,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None
        }
    );
}
