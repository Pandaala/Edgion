//! Body gate wrappers for body-aware plugins.
//!
//! Most plugins that need the fully buffered request body are wrapped in a body
//! gate. HMAC is the narrow exception: it owns the same resolved requirement so
//! cheap authentication can finish before capture, then hashes through the
//! non-materializing body view. Proxied upstream responses remain
//! streaming-only. The gate, evaluated at the plugin's stage:
//!
//! 1. Applies the SECONDARY run/skip filter ([`BodyRequirement::conditions`],
//!    re-using the same condition engine as the entry-level `conditions`). When
//!    it says "skip", the body is not required for THIS request and the inner
//!    plugin runs without body enforcement.
//! 2. Otherwise buffers the body to the SHARED resolved gateway-wide physical
//!    limit, not any per-plugin value, via the lazy
//!    [`PluginSession::ensure_request_body`]
//!    / `read_response_body` (OpenResty `ngx.req.read_body`-style), then applies
//!    the plugin's own `maxBodySize` LIMIT to the result (a body larger than the
//!    plugin's limit → too-large for THAT plugin only). The inner plugin reads
//!    the now-cached bytes only when its implementation needs them.
//! 3. On a refusal (`NotBuffered`) or gateway-wide physical cap
//!    (`BufferLimitExceeded`) rejects unconditionally: in both cases there is
//!    no forwarding copy of the original body, so fail-open is impossible.
//!    Recoverable read failures and per-plugin logical limits apply
//!    [`BodyReadFailureMode`].
//!
//! There is no protocol bypass. Which requests are bufferable is decided by the
//! read adapter, not here — see the body-gate admission model design.
//!
//! Wrapping order is `Conditional( BodyGate( inner ) )`: the entry-level
//! run_if/skip_if is evaluated first, so a plugin skipped by its own conditions
//! never demands the body.

use async_trait::async_trait;

use super::log::PluginLog;
use super::traits::session::BodyReadError;
use super::traits::{PluginSession, RequestFilter, UpstreamResponse};
use crate::plugins::runtime::conditions::{EvaluationResult, PluginConditionsExt};
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{BodyReadFailureMode, BodyRequirement};
use edgion_resources::EdgionStatus;

/// How much to physically buffer: the resolved gateway-wide body limit. This is a
/// SHARED per-request size — it does not depend on any per-plugin `maxBodySize`
/// (that is a per-plugin limit, not a buffer knob).
fn buffer_cap(session: &dyn PluginSession) -> u64 {
    session.ctx().request_body_config.max_body_size_bytes
}

/// The per-plugin body-size limit in bytes, if set and valid. A zero/unparseable
/// value was already surfaced at preparse, so it is ignored here (treated as no
/// limit) rather than rejecting all traffic.
fn plugin_limit_bytes(requirement: &BodyRequirement) -> Option<u64> {
    match requirement.max_body_size_bytes() {
        Some(Ok(v)) if v > 0 => Some(v),
        _ => None,
    }
}

/// Apply the per-plugin `maxBodySize` limit to an already-captured body: if the
/// body is larger than the plugin's own limit, that plugin (and only that
/// plugin) treats it as too-large. `None` = within the limit / no limit, run
/// the plugin.
pub(crate) fn over_plugin_limit_len(requirement: &BodyRequirement, body_len: u64) -> Option<BodyReadError> {
    plugin_limit_bytes(requirement).and_then(|limit| (body_len > limit).then_some(BodyReadError::TooLarge { limit }))
}

/// Wrapper that force-buffers the request body before running the inner filter.
pub struct BodyGatedRequestFilter {
    inner: Box<dyn RequestFilter>,
    requirement: BodyRequirement,
}

impl BodyGatedRequestFilter {
    pub fn new(inner: Box<dyn RequestFilter>, requirement: BodyRequirement) -> Self {
        Self { inner, requirement }
    }
}

#[async_trait]
impl RequestFilter for BodyGatedRequestFilter {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn alias(&self) -> Option<&str> {
        self.inner.alias()
    }

    async fn run_request(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        // 1. Secondary gate: is the body required for this request?
        if let Some(conds) = &self.requirement.conditions {
            if conds.should_evaluate() {
                let eval = conds.evaluate_detail(session, log).await;
                if eval.result == EvaluationResult::Skip {
                    // Body not required — run the plugin without body enforcement.
                    return self.inner.run_request(session, log).await;
                }
            }
        }

        // No CONNECT check: `pingora-proxy` answers CONNECT with 405 in
        // `handle_new_request`, before `process_request`, gated on
        // `HttpServerOptions::allow_connect_method_proxying` — which defaults
        // false and Edgion never sets. Verified for plain and RFC 8441 h2
        // extended CONNECT. Do not set that option without revisiting this.

        // 2. Buffer the body to the shared (gateway-wide) size without
        //    materializing a disk spill, then apply this plugin's own
        //    `maxBodySize` limit. The inner plugin reads the cached bytes only
        //    when its implementation needs them.
        let cap = buffer_cap(session);
        let capture_result = session.ensure_request_body(Some(cap)).await;
        match capture_result {
            Ok(body_len) => match over_plugin_limit_len(&self.requirement, body_len) {
                Some(err) => body_read_failure_result(session, &self.requirement, log, &err),
                None => self.inner.run_request(session, log).await,
            },
            Err(err) => body_read_failure_result(session, &self.requirement, log, &err),
        }
    }
}

/// Fail-closed backstop for an invalid response body gate.
///
/// Configuration validation rejects every such gate because proxied upstream
/// responses are streaming-only. This wrapper remains defensive runtime
/// handling if invalid configuration reaches the data plane.
pub struct BodyGatedUpstreamResponse {
    inner: Box<dyn UpstreamResponse>,
    requirement: BodyRequirement,
}

impl BodyGatedUpstreamResponse {
    pub fn new(inner: Box<dyn UpstreamResponse>, requirement: BodyRequirement) -> Self {
        Self { inner, requirement }
    }
}

#[async_trait]
impl UpstreamResponse for BodyGatedUpstreamResponse {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn alias(&self) -> Option<&str> {
        self.inner.alias()
    }

    async fn run_upstream_response(&self, session: &mut dyn PluginSession, log: &mut PluginLog) -> PluginRunningResult {
        if let Some(conds) = &self.requirement.conditions {
            if conds.should_evaluate() {
                let eval = conds.evaluate_detail(session, log).await;
                if eval.result == EvaluationResult::Skip {
                    return self.inner.run_upstream_response(session, log).await;
                }
            }
        }

        let cap = buffer_cap(session);
        let read_result = session.read_response_body(Some(cap)).await;
        match read_result {
            Ok(body) => match over_plugin_limit_len(&self.requirement, body.len() as u64) {
                Some(err) => body_read_failure_result(session, &self.requirement, log, &err),
                None => self.inner.run_upstream_response(session, log).await,
            },
            Err(err) => body_read_failure_result(session, &self.requirement, log, &err),
        }
    }
}

/// Resolve a failed body read into a plugin result.
///
/// Two axes, deliberately separate:
///
/// - A **refusal** (`NotBuffered`) is a statement that this request class is
///   outside what the gateway supports. It is ALWAYS fail-close and is never
///   routed through `on_read_failure` — tolerating it would mean running an
///   inspection plugin that inspected nothing, which no configuration should
///   be able to express. The precise reason is already on `ctx` as an
///   `EdgionStatus`, recorded by the read adapter.
/// - A gateway-wide **physical cap** (`BufferLimitExceeded`) is also always
///   fail-close: capture has been discarded and the original body cannot be
///   forwarded.
/// - A **read failure** (`TooLarge` / `Incomplete` / `Timeout` /
///   `Unimplemented`) is a runtime condition an operator may reasonably choose
///   to tolerate, so it keeps the `on_read_failure` knob. Oversize maps to 413
///   and a read timeout to 408 (client errors); everything else to 500.
pub(crate) fn body_read_failure_result(
    session: &mut dyn PluginSession,
    requirement: &BodyRequirement,
    log: &mut PluginLog,
    err: &BodyReadError,
) -> PluginRunningResult {
    if matches!(err, BodyReadError::NotBuffered) {
        // The adapter already recorded WHICH refusal on ctx; do not double-count.
        log.push_err("body-refused");
        return PluginRunningResult::ErrResponse {
            status: 500,
            body: None,
        };
    }

    if matches!(err, BodyReadError::BufferLimitExceeded { .. }) {
        session.add_error(EdgionStatus::BodyReadTooLarge);
        log.push_err("body-buffer-limit");
        return PluginRunningResult::ErrResponse {
            status: 413,
            body: None,
        };
    }

    // R17: a read failure is alertable too, and only `ctx.add_error` reaches the
    // access log — a `PluginLog` line alone stays inside a nested per-plugin array.
    session.add_error(match err {
        BodyReadError::TooLarge { .. } => EdgionStatus::BodyReadTooLarge,
        BodyReadError::BufferLimitExceeded { .. } => EdgionStatus::BodyReadTooLarge,
        BodyReadError::Timeout => EdgionStatus::BodyReadTimeout,
        BodyReadError::Incomplete => EdgionStatus::BodyReadAborted,
        BodyReadError::Unimplemented => EdgionStatus::BodyReadUnimplemented,
        // Handled above; unreachable.
        BodyReadError::NotBuffered => EdgionStatus::BodyRefusedStage,
    });

    match requirement.on_read_failure {
        BodyReadFailureMode::FailClose => {
            let status = match err {
                BodyReadError::TooLarge { .. } => 413,
                BodyReadError::Timeout => 408,
                _ => 500,
            };
            log.push_err("body-read-failed");
            PluginRunningResult::ErrResponse { status, body: None }
        }
        BodyReadFailureMode::FailOpen => {
            log.push_err("body-read-failed-open");
            PluginRunningResult::Nothing
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::runtime::traits::session::{BodyReadError, MockPluginSession};
    use crate::EdgionHttpContext;
    use bytes::Bytes;
    use edgion_resources::common::KeyGet;
    use edgion_resources::resources::edgion_plugins::conditions::{Condition, IncludeCondition, PluginConditions};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A mock session that carries a default ctx (so cap resolution has what
    /// it needs). Tests layer an `ensure_request_body` expectation on top.
    fn base_session() -> MockPluginSession {
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(EdgionHttpContext::new());
        session.expect_add_error().returning(|_| ());
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

    // The default session delegates ensure_request_body to read_request_body,
    // which returns Unimplemented.
    #[tokio::test]
    async fn fail_close_returns_500_and_skips_inner() {
        let inner = CountingFilter::new();
        let gate = BodyGatedRequestFilter::new(Box::new(inner), BodyRequirement::fail_close());

        let mut session = base_session();
        session
            .expect_ensure_request_body()
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
            .expect_ensure_request_body()
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
            .expect_ensure_request_body()
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
            .expect_ensure_request_body()
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
        session.expect_ensure_request_body().returning(|_| Ok(2));

        let mut log = PluginLog::new("test");
        let result = gate.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn secondary_skip_runs_inner_without_buffering() {
        let inner = CountingFilter::new();
        // Secondary run-condition: method must be POST. Request is GET → skip →
        // body not required → inner runs, ensure_request_body is never called.
        let req = BodyRequirement {
            conditions: Some(PluginConditions {
                skip: None,
                run: Some(vec![Condition::Include(IncludeCondition {
                    key: KeyGet::Method,
                    values: Some(vec!["POST".to_string()]),
                    regex: None,
                    values_set: None,
                    wildcard_patterns: None,
                    compiled_regex: None,
                })]),
            }),
            ..Default::default()
        };
        let gate = BodyGatedRequestFilter::new(Box::new(inner), req);

        let mut session = MockPluginSession::new();
        session.expect_key_get().return_const(Some("GET".to_string()));
        // ensure_request_body must NOT be called; no expectation set.

        let mut log = PluginLog::new("test");
        let result = gate.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
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
            run: Some(vec![Condition::Include(IncludeCondition {
                key: KeyGet::Method,
                values: None,
                regex: Some(vec!["^POST$".to_string()]),
                values_set: None,
                wildcard_patterns: None,
                compiled_regex: None,
            })]),
        };
        conds.compile().expect("regex compiles");
        let req = BodyRequirement {
            conditions: Some(conds),
            ..BodyRequirement::fail_close()
        };
        let gate = BodyGatedRequestFilter::new(Box::new(CountingFilter::new()), req);

        let mut session = base_session();
        session.expect_key_get().return_const(Some("POST".to_string()));
        session
            .expect_ensure_request_body()
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
        session.expect_ensure_request_body().returning(|_| Ok(100));

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
        session.expect_ensure_request_body().returning(|_| Ok(5));

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
        session.expect_ensure_request_body().returning(|_| Ok(100));

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
                .expect_ensure_request_body()
                .returning(move |_| Ok(body.len() as u64));
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
            .expect_ensure_request_body()
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
            .expect_ensure_request_body()
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
            .expect_ensure_request_body()
            .returning(|_| Err(BodyReadError::Timeout));

        let mut log = PluginLog::new("test");
        assert_eq!(
            gate.run_request(&mut session, &mut log).await,
            PluginRunningResult::Nothing
        );
    }

    // ===== Pin the BodyReadError -> EdgionStatus mapping =====
    //
    // `base_session()`'s `expect_add_error().returning(|_| ())` accepts any
    // status, so it cannot catch a swapped arm in `body_read_failure_result`.
    // These tests build a session mock local to each case with a `.withf(...)`
    // predicate on the exact expected `EdgionStatus`, so a swapped mapping fails
    // the test instead of silently passing.

    /// A session mock like `base_session()`, but with a strict `add_error`
    /// expectation pinned to `expected` (called exactly once) instead of the
    /// permissive any-status stub.
    fn session_expecting_error(expected: EdgionStatus) -> MockPluginSession {
        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(EdgionHttpContext::new());
        session
            .expect_add_error()
            .withf(move |status| *status == expected)
            .times(1)
            .returning(|_| ());
        session
    }

    #[tokio::test]
    async fn too_large_records_body_read_too_large_status() {
        let gate = BodyGatedRequestFilter::new(Box::new(CountingFilter::new()), BodyRequirement::fail_close());
        let mut session = session_expecting_error(EdgionStatus::BodyReadTooLarge);
        session
            .expect_ensure_request_body()
            .returning(|_| Err(BodyReadError::TooLarge { limit: 1024 }));

        let mut log = PluginLog::new("test");
        gate.run_request(&mut session, &mut log).await;
    }

    #[tokio::test]
    async fn timeout_records_body_read_timeout_status() {
        let gate = BodyGatedRequestFilter::new(Box::new(CountingFilter::new()), BodyRequirement::fail_close());
        let mut session = session_expecting_error(EdgionStatus::BodyReadTimeout);
        session
            .expect_ensure_request_body()
            .returning(|_| Err(BodyReadError::Timeout));

        let mut log = PluginLog::new("test");
        gate.run_request(&mut session, &mut log).await;
    }

    #[tokio::test]
    async fn incomplete_records_body_read_aborted_status() {
        let gate = BodyGatedRequestFilter::new(Box::new(CountingFilter::new()), BodyRequirement::fail_close());
        let mut session = session_expecting_error(EdgionStatus::BodyReadAborted);
        session
            .expect_ensure_request_body()
            .returning(|_| Err(BodyReadError::Incomplete));

        let mut log = PluginLog::new("test");
        gate.run_request(&mut session, &mut log).await;
    }

    #[tokio::test]
    async fn unimplemented_records_body_read_unimplemented_status() {
        let gate = BodyGatedRequestFilter::new(Box::new(CountingFilter::new()), BodyRequirement::fail_close());
        let mut session = session_expecting_error(EdgionStatus::BodyReadUnimplemented);
        session
            .expect_ensure_request_body()
            .returning(|_| Err(BodyReadError::Unimplemented));

        let mut log = PluginLog::new("test");
        gate.run_request(&mut session, &mut log).await;
    }

    #[tokio::test]
    async fn not_buffered_never_reaches_add_error() {
        // The read adapter already recorded the specific refusal reason on ctx;
        // the gate deliberately does not double-count it via `add_error`.
        let gate = BodyGatedRequestFilter::new(Box::new(CountingFilter::new()), BodyRequirement::fail_close());

        let mut session = MockPluginSession::new();
        session.expect_ctx().return_const(EdgionHttpContext::new());
        session.expect_add_error().times(0);
        session
            .expect_ensure_request_body()
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
}
