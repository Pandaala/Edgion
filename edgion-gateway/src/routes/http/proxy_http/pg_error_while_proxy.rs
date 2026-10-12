use super::pg_upstream_response_filter::has_final_response_written;
use super::EdgionHttpProxy;
use crate::ai::http::attempt::{self, RetryTransportFacts};
use crate::{EdgionHttpContext, PeerTimeoutSource};
use edgion_resources::EdgionErrorCode;
use pingora_core::prelude::HttpPeer;
use pingora_core::{Error, ErrorSource, ErrorType};
use pingora_proxy::Session;

#[inline]
fn should_record_local_origin_failure(
    retry_on_status: bool,
    error_type: &ErrorType,
    error_source: &ErrorSource,
) -> bool {
    !retry_on_status
        && !matches!(error_type, ErrorType::HTTPStatus(_))
        && !matches!(error_source, ErrorSource::Downstream)
}

#[inline]
fn ambiguous_decided_h2_retry_allowed(
    error: &Error,
    replayable: bool,
    method_allowed: bool,
    attempts_allowed: bool,
    deadline_exceeded: bool,
) -> Option<bool> {
    (matches!(error.retry, pingora_core::RetryType::Decided(true))
        && matches!(error.etype(), ErrorType::InvalidH2 | ErrorType::H2Downgrade))
    .then_some(replayable && method_allowed && attempts_allowed && !deadline_exceeded)
}

#[inline]
fn is_backend_request_timeout_retry_candidate(
    error: &Error,
    timeout_source: Option<PeerTimeoutSource>,
    retry_configured: bool,
    session: &Session,
) -> bool {
    matches!(error.etype(), ErrorType::ReadTimedout | ErrorType::WriteTimedout)
        && matches!(error.esource(), ErrorSource::Upstream)
        && timeout_source == Some(PeerTimeoutSource::BackendRequest)
        && retry_configured
        && !has_final_response_written(session)
}

/// Fail-safe ceiling for retries Pingora decided on its own, i.e. failures that
/// reach this callback already carrying `RetryType::Decided(true)`: h2 GOAWAY,
/// RST_STREAM(REFUSED_STREAM), an HTTP/1.1-required downgrade, or a
/// reused-connection IO error. Those decisions are made inside the upstream h2
/// client. Ambiguous protocol errors and AI failures are subject to the gates
/// below; protocol-qualified safe retries may still bypass the route budget.
///
/// `ServerConf.max_retries` used to bound them at its default of 16. It is now
/// sized for the public route budget (`1 + attempts`, up to `i32::MAX`), so
/// without a dedicated ceiling an upstream that answers every stream with
/// RST_STREAM(REFUSED_STREAM) spins the proxy loop until the downstream client
/// gives up. This restores the former bound for exactly that class of retry.
const MAX_PINGORA_DECIDED_RETRIES: u32 = 16;

#[inline]
fn pingora_decided_retry_exhausted(decided_retries: u32) -> bool {
    decided_retries > MAX_PINGORA_DECIDED_RETRIES
}

/// Applies Edgion's fail-safe ceiling on top of the ordinary retry decision.
///
/// Only retries Pingora decided for us are counted and clamped; a retry this
/// file set explicitly has already passed the route or AI budget gates and is
/// left alone.
#[inline]
pub fn error_while_proxy(
    _edgion_http: &EdgionHttpProxy,
    peer: &HttpPeer,
    session: &mut Session,
    e: Box<Error>,
    ctx: &mut EdgionHttpContext,
    client_reused: bool,
) -> Box<Error> {
    let pingora_decided_retry = matches!(e.retry, pingora_core::RetryType::Decided(true));
    if pingora_decided_retry {
        ctx.pingora_decided_retries = ctx.pingora_decided_retries.saturating_add(1);
    }

    let mut e = decide_retry(peer, session, e, ctx, client_reused);

    if pingora_decided_retry && e.retry() && pingora_decided_retry_exhausted(ctx.pingora_decided_retries) {
        ctx.record_error(EdgionErrorCode::UpstreamTransportRetryCeiling);
        e.set_retry(false);
    }

    e
}

#[inline]
fn decide_retry(
    peer: &HttpPeer,
    session: &mut Session,
    e: Box<Error>,
    ctx: &mut EdgionHttpContext,
    client_reused: bool,
) -> Box<Error> {
    // filter-04b: the sentinel error from a written local reply is intentional,
    // not an upstream failure. Skip body-size accounting and outlier-detection
    // failure recording, and leave the error non-retryable.
    if ctx.local_reply_written() {
        return e.more_context(format!("Peer: {}", peer));
    }

    if let Some(upstream) = ctx.get_current_upstream_mut() {
        upstream.set_response_body_size(session.upstream_body_bytes_received());
    }
    let mut e = e.more_context(format!("Peer: {}", peer));

    // Record only backend-side transport/proxy failures as local-origin.
    // Downstream failures belong to the client connection and must not affect
    // backend health. HTTPStatus errors
    // are control flow for an already-observed upstream response (status retry
    // or retry-budget denial), or a locally generated response status; the
    // response filter has already classified any real upstream status.
    if should_record_local_origin_failure(ctx.retry_on_status, e.etype(), e.esource()) {
        if let Some(upstream) = ctx.get_current_upstream() {
            if let (Some(backend), Some(token)) =
                (upstream.lb_backend.as_deref(), upstream.lb_outlier_completion.as_ref())
            {
                // Mid-proxy read/write error or transport reset — local-origin failure.
                crate::backends::get_outlier_detection_store().record_failure_for_backend_with_token(
                    backend,
                    token,
                    crate::backends::FailureClass::LocalOrigin,
                );
            }
        }
    }

    // A bounded response-head processor has already selected this final
    // attempt, or a final head has reached the writer. Pingora will also veto
    // retry after this callback, but the gate must be visible here before AI
    // dispatch consumes retry budget, advances the scheduler, or settles a
    // successor attempt. Keep the accounting above, then return the final
    // non-retryable error without entering any retry decision path.
    if session.response_head_retry_closed() {
        e.set_retry(false);
        return e;
    }

    // Pingora combines the frozen structural policy with the live backing
    // state. This distinguishes "never enabled" from a usable native buffer,
    // registered replay, and truncation.
    let buffer_ok = session.request_relay_retry_state().can_start_next_attempt();

    if ctx.has_ai_attempt_plan() {
        return attempt::on_transport_failure(
            ctx,
            RetryTransportFacts {
                relay_allows_retry: buffer_ok,
                final_response_written: has_final_response_written(session),
            },
            e,
        );
    }

    // Case 1: status code triggered retry (set by upstream_response_filter)
    // Retry budget and timeout already checked in upstream_response_filter
    if ctx.retry_on_status && buffer_ok {
        ctx.retry_on_status = false; // reset for next attempt
        e.set_retry(true);
        return e;
    }

    // Case 1.5: per-try timeout (GEP-1731). An UPSTREAM read/write timeout
    // whose operative bound was `backendRequest` cancels the attempt; when the
    // rule carries a retry policy, the attempt is retried within the usual
    // budget/deadline/buffer guards. Timeouts sourced from the route `request`
    // deadline or the client `grpc-timeout` deadline are never retried — the
    // caller is out of budget. Downstream-sourced timeouts (slow client) are
    // excluded: retrying the upstream cannot help them.
    // Also requires that no final response has been written downstream yet:
    // relayed 100/102/103 headers still permit a later final response, while a
    // 101 upgrade or ordinary final header closes retry.
    if is_backend_request_timeout_retry_candidate(&e, ctx.peer_timeout_source, ctx.retry_configured(), session) {
        let retry_attempts = ctx.retry_attempts().unwrap_or(ctx.egc.retry_attempts);
        let deadline_exceeded = ctx.request_timeout_exceeded() || ctx.grpc_deadline_exceeded();
        if buffer_ok && super::retry_budget_allows_next_attempt(ctx.try_cnt, retry_attempts) && !deadline_exceeded {
            // Stamp the canceled attempt's per-upstream log entry so it is
            // self-describing (mirrors pg_fail_to_connect): without this the
            // entry shows neither status nor error and operators must infer
            // the cancellation from the request-level diagnostics.
            if let Some(upstream) = ctx.get_current_upstream_mut() {
                upstream.status = Some(504);
                upstream.err.push(e.etype().as_str().to_string());
                upstream.et = Some(
                    std::time::Instant::now()
                        .saturating_duration_since(upstream.start_time)
                        .as_millis() as u64,
                );
            }
            // Retry budget (retryConstraint): composes AFTER the attempts/
            // deadline/buffer checks of this Case 1.5 (D3) — both gates must pass.
            // On refusal the retry flag is NOT set; instead the client gets a
            // deterministic 503 by swapping the error's ErrorType (pg_fail_to_proxy
            // renders the status from the error). The per-upstream entry keeps its
            // real observed 504 timeout status (stamped just above); the 503 is a
            // request-level outcome carried by the error, and BackendRequestTimeoutRetry
            // is intentionally NOT added because no retry actually occurs.
            if let Some(budget) = super::pg_upstream_peer::retry_budget_for(ctx) {
                if !budget.try_retry() {
                    crate::observe::metrics::record_retry_budget_denied();
                    ctx.record_error(EdgionErrorCode::RetryBudgetExhausted);
                    return Error::explain(ErrorType::HTTPStatus(503), "retry budget exhausted");
                }
            }
            ctx.record_error(EdgionErrorCode::BackendRequestTimeoutRetry);
            e.set_retry(true);
            return e;
        }
    }

    // Case 2: connection reuse retry (Pingora built-in: stale keepalive connection)
    let retry_attempts = ctx.retry_attempts().unwrap_or(ctx.egc.retry_attempts);

    // Either route `request_timeout` or client `grpc-timeout` having
    // expired disqualifies further retries.
    let deadline_exceeded = ctx.request_timeout_exceeded() || ctx.grpc_deadline_exceeded();

    // Method-safety guard (route-01): a reused keepalive connection reset
    // mid-request replays the buffered request body on a fresh connection. For
    // non-idempotent methods (POST/PATCH/CONNECT, and all gRPC since it is POST
    // at the HTTP layer) that replay risks a silent duplicate mutation, so it is
    // suppressed by default. Idempotent methods (GET/HEAD/PUT/DELETE/OPTIONS/
    // TRACE) are always safe to replay. A route may opt back in via the
    // `edgion.io/retry-non-idempotent: "true"` annotation. Only the
    // reused-connection path needs this: `fail_to_connect` never wrote any
    // request bytes upstream and is therefore always safe to retry.
    let method_retry_ok = session.req_header().method.is_idempotent() || ctx.retry_allow_non_idempotent();

    let can_retry = client_reused
        && buffer_ok
        && method_retry_ok
        && super::retry_budget_allows_next_attempt(ctx.try_cnt, retry_attempts)
        && !deadline_exceeded;

    // A local H2 protocol error or downgrade does not prove that the origin
    // skipped the request. Pingora may arrive with Decided(true), which
    // decide_reuse cannot revoke. Apply the same replay safety gates to these
    // ambiguous outcomes even when the H2 connection was fresh.
    if ambiguous_decided_h2_retry_allowed(
        &e,
        buffer_ok,
        method_retry_ok,
        super::retry_budget_allows_next_attempt(ctx.try_cnt, retry_attempts),
        deadline_exceeded,
    ) == Some(false)
    {
        e.set_retry(false);
    }

    // Capture whether the retry decision is ours to make: errors that arrive
    // already Decided(true) (e.g. h2 GOAWAY / REFUSED_STREAM decided inside
    // Pingora) must not be labeled as reuse retries.
    let was_undecided = matches!(e.retry, pingora_core::RetryType::ReusedOnly);
    e.retry.decide_reuse(can_retry);

    // Stamp the attempt when OUR reuse decision accepts the retry so the extra
    // upstream entry in the access log carries an explicit reason instead of
    // an unexplained second attempt.
    if was_undecided && e.retry() {
        if let Some(upstream) = ctx.get_current_upstream_mut() {
            upstream.err.push("ConnectionReuseRetry".to_string());
        }
    }

    e
}

#[cfg(test)]
#[path = "../../../../tests/unit/routes/http/proxy_http/pg_error_while_proxy_test.rs"]
mod tests;
