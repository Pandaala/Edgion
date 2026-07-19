//! Body gate wrappers for body-aware plugins.
//!
//! A plugin that needs the fully-buffered request/response body is wrapped in a
//! body gate. The gate, evaluated at the plugin's stage:
//!
//! 1. Applies the SECONDARY run/skip filter ([`BodyRequirement::conditions`],
//!    re-using the same condition engine as the entry-level `conditions`). When
//!    it says "skip", the body is not required for THIS request and the inner
//!    plugin runs without body enforcement.
//! 2. Otherwise buffers the body to the SHARED gateway-wide size (`maxBodySize`,
//!    not any per-plugin value) via the lazy [`PluginSession::read_request_body`]
//!    / `read_response_body` (OpenResty `ngx.req.read_body`-style), then applies
//!    the plugin's own `maxBodySize` LIMIT to the result (a body larger than the
//!    plugin's limit → too-large for THAT plugin only). The inner plugin re-reads
//!    the now-cached bytes.
//! 3. On a read failure (or over-limit) applies [`BodyReadFailureMode`]:
//!    `FailClose` (default) rejects with `500` (or `413` if the body was too
//!    large); `FailOpen` skips the plugin as if it were disabled.
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

/// True for a tunneled/upgrade request whose body stream never delivers
/// end-of-stream — buffering it would deadlock. Covers both `Upgrade:` (HTTP/1.1
/// WebSocket) and `CONNECT` (plain tunnel + RFC 8441 h2 extended CONNECT, which
/// carries NO `Upgrade` header). Mirrors `session_adapter.rs`'s upgrade test.
fn is_tunnel_request(session: &dyn PluginSession) -> bool {
    session.get_method().eq_ignore_ascii_case("CONNECT") || session.header_value("upgrade").is_some()
}

/// How much to physically buffer: the gateway-wide `maxBodySize`. This is a
/// SHARED per-request size — it does not depend on any per-plugin `maxBodySize`
/// (that is a per-plugin limit, not a buffer knob).
fn buffer_cap(session: &dyn PluginSession) -> u64 {
    session.ctx().max_body_size_bytes
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

/// Apply the per-plugin `maxBodySize` limit to an already-read body: if the body
/// is larger than the plugin's own limit, that plugin (and only that plugin)
/// treats it as too-large. `None` = within the limit / no limit, run the plugin.
fn over_plugin_limit(requirement: &BodyRequirement, body: &bytes::Bytes) -> Option<BodyReadError> {
    plugin_limit_bytes(requirement)
        .and_then(|limit| (body.len() as u64 > limit).then_some(BodyReadError::TooLarge { limit }))
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

        // 2. WebSocket/Upgrade/CONNECT bypass: forcing a full-body buffer on a
        //    tunneled request would deadlock (end-of-stream never arrives). Skip
        //    buffering and run the plugin without it, regardless of fail policy.
        if is_tunnel_request(session) {
            log.push("upgrade/connect request, body buffering skipped");
            return self.inner.run_request(session, log).await;
        }

        // 3. Buffer the body to the shared (gateway-wide) size, then apply this
        //    plugin's own `maxBodySize` limit. The inner plugin re-reads the
        //    now-cached bytes.
        let cap = buffer_cap(session);
        match session.read_request_body(Some(cap)).await {
            Ok(body) => match over_plugin_limit(&self.requirement, &body) {
                Some(err) => body_read_failure_result(&self.requirement, log, &err),
                None => self.inner.run_request(session, log).await,
            },
            Err(err) => body_read_failure_result(&self.requirement, log, &err),
        }
    }
}

/// Wrapper that force-buffers the response body before running the inner handler.
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

        // WebSocket/Upgrade/CONNECT bypass (symmetric with the request gate): a
        // tunneled exchange never delivers end-of-stream, so buffering the
        // response body would deadlock. Skip buffering and run the plugin.
        if is_tunnel_request(session) {
            log.push("upgrade/connect request, response body buffering skipped");
            return self.inner.run_upstream_response(session, log).await;
        }

        let cap = buffer_cap(session);
        match session.read_response_body(Some(cap)).await {
            Ok(body) => match over_plugin_limit(&self.requirement, &body) {
                Some(err) => body_read_failure_result(&self.requirement, log, &err),
                None => self.inner.run_upstream_response(session, log).await,
            },
            Err(err) => body_read_failure_result(&self.requirement, log, &err),
        }
    }
}

/// Record the read failure on the plugin log and resolve the fail-open/close
/// policy into the plugin result. Fail-open → `Nothing` (skip). Fail-close →
/// `ErrResponse`: an oversize body maps to `413` (client error, matching
/// Envoy/Kong); every other reason maps to `500`.
fn body_read_failure_result(
    requirement: &BodyRequirement,
    log: &mut PluginLog,
    err: &BodyReadError,
) -> PluginRunningResult {
    match requirement.on_read_failure {
        BodyReadFailureMode::FailClose => {
            let status = match err {
                BodyReadError::TooLarge { .. } => 413,
                _ => 500,
            };
            log.push(&format!("body read failed ({err}), fail-close ({status})"));
            PluginRunningResult::ErrResponse { status, body: None }
        }
        BodyReadFailureMode::FailOpen => {
            log.push(&format!("body read failed ({err}), fail-open (skipped)"));
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

    /// A mock session that is not a tunnel/upgrade request and carries a default
    /// ctx (so the body gate's tunnel check and cap resolution have what they
    /// need). Tests layer a `read_request_body` expectation on top.
    fn base_session() -> MockPluginSession {
        let mut session = MockPluginSession::new();
        session.expect_header_value().returning(|_| None);
        session.expect_get_method().return_const("GET".to_string());
        session.expect_ctx().return_const(EdgionHttpContext::new());
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
    async fn upgrade_request_skips_buffering() {
        let inner = CountingFilter::new();
        let gate = BodyGatedRequestFilter::new(Box::new(inner), BodyRequirement::fail_close());

        // Upgrade request → the gate must skip buffering entirely (no
        // read_request_body expectation is set, so any call would panic).
        let mut session = MockPluginSession::new();
        session.expect_get_method().return_const("GET".to_string());
        session
            .expect_header_value()
            .returning(|name| (name == "upgrade").then(|| "websocket".to_string()));

        let mut log = PluginLog::new("test");
        let result = gate.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn connect_request_skips_buffering() {
        // CONNECT (incl. h2 extended CONNECT) carries no Upgrade header but must
        // still bypass buffering — read_request_body is never expected.
        let gate = BodyGatedRequestFilter::new(Box::new(CountingFilter::new()), BodyRequirement::fail_close());
        let mut session = MockPluginSession::new();
        session.expect_get_method().return_const("CONNECT".to_string());
        session.expect_header_value().returning(|_| None);

        let mut log = PluginLog::new("test");
        let result = gate.run_request(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn response_gate_upgrade_skips_buffering() {
        let gate = response_gate();
        // read_response_body is never expected → any call panics.
        let mut session = MockPluginSession::new();
        session.expect_get_method().return_const("GET".to_string());
        session
            .expect_header_value()
            .returning(|name| (name == "upgrade").then(|| "websocket".to_string()));

        let mut log = PluginLog::new("test");
        let result = gate.run_upstream_response(&mut session, &mut log).await;
        assert_eq!(result, PluginRunningResult::GoodNext);
    }

    #[tokio::test]
    async fn secondary_skip_runs_inner_without_buffering() {
        let inner = CountingFilter::new();
        // Secondary run-condition: method must be POST. Request is GET → skip →
        // body not required → inner runs, read_request_body is never called.
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
        // read_request_body must NOT be called; no expectation set.

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
}
