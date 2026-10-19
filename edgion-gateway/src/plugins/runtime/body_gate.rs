//! Body gate wrappers for body-aware plugins.
//!
//! Most plugins that need the fully buffered request body are wrapped in a body
//! gate. HMAC owns the same resolved requirement so cheap authentication can
//! finish before capture, then hashes through the non-materializing body view.
//! GrpcJsonTranscoder owns capture admission to preserve its fixed JSON error
//! dialect, using the same shared failure classification. Proxied upstream responses remain
//! streaming-only. The gate, evaluated at the plugin's stage:
//!
//! 1. Applies the SECONDARY run/skip filter ([`BodyRequirement::conditions`],
//!    re-using the same condition engine as the entry-level `conditions`). When
//!    it says "skip", the body is not required for THIS request and the inner
//!    plugin runs without body enforcement.
//! 2. Otherwise buffers the body to the SHARED resolved gateway-wide physical
//!    limit, not any per-plugin value. Filters require full materialization by
//!    default so storage-read failures retain the gate's failure policy.
//!    Capture-only filters such as `RequestBodyBuffer` explicitly opt out and
//!    use [`PluginSession::ensure_request_body`]. The gate then applies the
//!    plugin's own `maxBodySize` LIMIT to the exact captured length.
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
use super::traits::{PluginSession, RequestFilter, UpstreamResponse};
use crate::plugins::runtime::conditions::{CompiledPluginConditions, EvaluationResult};
use crate::request_body::BodyReadError;
use edgion_resources::filters::PluginRunningResult;
use edgion_resources::resources::edgion_plugins::{BodyReadFailureMode, BodyRequirement};
use edgion_resources::EdgionErrorCode;

/// How much to physically buffer: the resolved gateway-wide body limit. This is a
/// SHARED per-request size — it does not depend on any per-plugin `maxBodySize`
/// (that is a per-plugin limit, not a buffer knob).
fn buffer_cap(session: &dyn PluginSession) -> u64 {
    session.ctx().request_body_config.max_body_size_bytes
}

/// Resolve the per-plugin body-size limit once during runtime construction.
/// Returns bytes if set and valid. A zero/unparseable
/// value was already surfaced at preparse, so it is ignored here (treated as no
/// limit) rather than rejecting all traffic.
pub(crate) fn plugin_limit_bytes(requirement: &BodyRequirement) -> Option<u64> {
    match requirement.max_body_size_bytes() {
        Some(Ok(v)) if v > 0 => Some(v),
        _ => None,
    }
}

/// Apply an already-resolved per-plugin `maxBodySize` limit to a captured body: if the
/// body is larger than the plugin's own limit, that plugin (and only that
/// plugin) treats it as too-large. `None` = within the limit / no limit, run
/// the plugin.
pub(crate) fn over_plugin_limit_len(limit_bytes: Option<u64>, body_len: u64) -> Option<BodyReadError> {
    limit_bytes.and_then(|limit| (body_len > limit).then_some(BodyReadError::TooLarge { limit }))
}

/// Compact, immutable policy used by a body gate at runtime.
struct BodyGatePolicy {
    logical_limit_bytes: Option<u64>,
    on_read_failure: BodyReadFailureMode,
    conditions: Option<CompiledPluginConditions>,
}

impl BodyGatePolicy {
    fn new(requirement: BodyRequirement, compiled: Option<CompiledPluginConditions>) -> Self {
        let logical_limit_bytes = plugin_limit_bytes(&requirement);
        let BodyRequirement { on_read_failure, .. } = requirement;
        Self {
            logical_limit_bytes,
            on_read_failure,
            conditions: compiled,
        }
    }

    fn over_limit_len(&self, body_len: u64) -> Option<BodyReadError> {
        over_plugin_limit_len(self.logical_limit_bytes, body_len)
    }

    fn body_read_failure_result(
        &self,
        session: &mut dyn PluginSession,
        log: &mut PluginLog,
        err: &BodyReadError,
    ) -> PluginRunningResult {
        body_read_failure_result_for_mode(session, self.on_read_failure, log, err)
    }
}

/// Record the canonical access-status signal for a body read failure.
///
/// `NotBuffered` is intentionally excluded because the read adapter records
/// the more precise refusal status when it rejects the request class.
pub(crate) fn record_body_read_error_status(session: &mut dyn PluginSession, err: &BodyReadError) {
    let status = match err {
        BodyReadError::TooLarge { .. } | BodyReadError::BufferLimitExceeded { .. } => EdgionErrorCode::BodyReadTooLarge,
        BodyReadError::Timeout => EdgionErrorCode::BodyReadTimeout,
        BodyReadError::Incomplete => EdgionErrorCode::BodyReadAborted,
        BodyReadError::StorageFailure => EdgionErrorCode::BodyCaptureStorageFailed,
        BodyReadError::GenerationConflict => EdgionErrorCode::BodyMutationConflict,
        BodyReadError::Unimplemented => EdgionErrorCode::BodyReadUnimplemented,
        BodyReadError::NotBuffered => return,
    };
    session.record_error(status);
}

/// Wrapper that force-buffers the request body before running the inner filter.
pub struct BodyGatedRequestFilter {
    inner: Box<dyn RequestFilter>,
    policy: BodyGatePolicy,
}

impl BodyGatedRequestFilter {
    pub(crate) fn with_conditions(
        inner: Box<dyn RequestFilter>,
        requirement: BodyRequirement,
        conditions: Option<CompiledPluginConditions>,
    ) -> Self {
        Self {
            inner,
            policy: BodyGatePolicy::new(requirement, conditions),
        }
    }

    pub fn new(inner: Box<dyn RequestFilter>, mut requirement: BodyRequirement) -> Self {
        let conditions = requirement.conditions.take().map(CompiledPluginConditions::new);
        Self {
            inner,
            policy: BodyGatePolicy::new(requirement, conditions),
        }
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
        if let Some(conds) = &self.policy.conditions {
            if conds.should_evaluate() && conds.evaluate(session) == EvaluationResult::Skip {
                // Body not required — run the plugin without body enforcement.
                return self.inner.run_request(session, log).await;
            }
        }

        // No CONNECT check: `pingora-proxy` answers CONNECT with 405 in
        // `handle_new_request`, before `process_request`, gated on
        // `HttpServerOptions::allow_connect_method_proxying` — which defaults
        // false and Edgion never sets. Verified for plain and RFC 8441 h2
        // extended CONNECT. Do not set that option without revisiting this.

        // 2. Buffer the body to the shared (gateway-wide) size, then apply this
        //    plugin's own `maxBodySize` limit. Full-byte filters materialize
        //    here so storage-read failures retain the gate's failure policy;
        //    capture-only filters explicitly preserve a disk spill.
        let cap = buffer_cap(session);
        let body_len_result = if self.inner.requires_materialized_request_body() {
            session.read_request_body(Some(cap)).await.map(|body| body.len() as u64)
        } else {
            session.ensure_request_body(Some(cap)).await
        };
        match body_len_result {
            Ok(body_len) => match self.policy.over_limit_len(body_len) {
                Some(err) => self.policy.body_read_failure_result(session, log, &err),
                None => self.inner.run_request(session, log).await,
            },
            Err(err) => self.policy.body_read_failure_result(session, log, &err),
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
    policy: BodyGatePolicy,
}

impl BodyGatedUpstreamResponse {
    pub(crate) fn with_conditions(
        inner: Box<dyn UpstreamResponse>,
        requirement: BodyRequirement,
        conditions: Option<CompiledPluginConditions>,
    ) -> Self {
        Self {
            inner,
            policy: BodyGatePolicy::new(requirement, conditions),
        }
    }

    pub fn new(inner: Box<dyn UpstreamResponse>, mut requirement: BodyRequirement) -> Self {
        let conditions = requirement.conditions.take().map(CompiledPluginConditions::new);
        Self {
            inner,
            policy: BodyGatePolicy::new(requirement, conditions),
        }
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
        if let Some(conds) = &self.policy.conditions {
            if conds.should_evaluate() && conds.evaluate(session) == EvaluationResult::Skip {
                return self.inner.run_upstream_response(session, log).await;
            }
        }

        let cap = buffer_cap(session);
        let read_result = session.read_response_body(Some(cap)).await;
        match read_result {
            Ok(body) => match self.policy.over_limit_len(body.len() as u64) {
                Some(err) => self.policy.body_read_failure_result(session, log, &err),
                None => self.inner.run_upstream_response(session, log).await,
            },
            Err(err) => self.policy.body_read_failure_result(session, log, &err),
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
///   `EdgionErrorCode`, recorded by the read adapter.
/// - A gateway-wide **physical cap** (`BufferLimitExceeded`) is also always
///   fail-close: capture has been discarded and the original body cannot be
///   forwarded.
/// - A **storage failure** (`StorageFailure`) is likewise always fail-close:
///   consumed bytes were never stored, so no forwarding copy exists. It gets
///   its own status (`BodyCaptureStorageFailed`) because it is an
///   infrastructure fault, not client behavior.
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
    body_read_failure_result_for_mode(session, requirement.on_read_failure, log, err)
}

fn body_read_failure_result_for_mode(
    session: &mut dyn PluginSession,
    on_read_failure: BodyReadFailureMode,
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
        record_body_read_error_status(session, err);
        log.push_err("body-buffer-limit");
        return PluginRunningResult::ErrResponse {
            status: 413,
            body: None,
        };
    }

    if matches!(err, BodyReadError::StorageFailure) {
        // The storage backend failed mid-capture: consumed bytes were never
        // stored, so no forwarding copy of the body exists and fail-open is
        // impossible — same reasoning as the physical cap above. Recorded
        // under its own status (not `BodyReadAborted`) because this is an
        // infrastructure fault (e.g. full disk), not a client abort.
        record_body_read_error_status(session, err);
        log.push_err("body-storage-failed");
        return PluginRunningResult::ErrResponse {
            status: 500,
            body: None,
        };
    }

    // R17: a read failure is alertable too, and only `ctx.record_error` reaches the
    // access log — a `PluginLog` line alone stays inside a nested per-plugin array.
    record_body_read_error_status(session, err);

    match on_read_failure {
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

// Unit tests for this module live in `tests/body_gate_test.rs`.
#[cfg(test)]
#[path = "../../../tests/unit/plugins/runtime/body_gate_test.rs"]
mod tests;
