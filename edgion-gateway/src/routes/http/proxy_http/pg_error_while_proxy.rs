use super::pg_upstream_response_filter::has_final_response_written;
use super::EdgionHttpProxy;
use crate::{EdgionHttpContext, PeerTimeoutSource};
use edgion_resources::EdgionErrorCode;
use pingora_core::prelude::HttpPeer;
use pingora_core::{Error, ErrorSource, ErrorType};
use pingora_proxy::Session;

#[inline]
fn should_record_local_origin_failure(retry_on_status: bool, error_type: &ErrorType) -> bool {
    !retry_on_status && !matches!(error_type, ErrorType::HTTPStatus(_))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AiProxyFailureClass {
    BackendRequestTimeout,
    AmbiguousUpstream,
}

fn ai_failure_wants_retry(
    class: AiProxyFailureClass,
    retry_configured: bool,
    retry_allow_non_idempotent: bool,
) -> bool {
    match class {
        AiProxyFailureClass::BackendRequestTimeout => retry_configured,
        AiProxyFailureClass::AmbiguousUpstream => retry_allow_non_idempotent,
    }
}

fn classify_ai_proxy_failure(
    error_type: &ErrorType,
    error_source: &ErrorSource,
    timeout_source: Option<PeerTimeoutSource>,
) -> Option<AiProxyFailureClass> {
    if !matches!(error_source, ErrorSource::Upstream) {
        return None;
    }
    match error_type {
        ErrorType::ReadTimedout | ErrorType::WriteTimedout => (timeout_source
            == Some(PeerTimeoutSource::BackendRequest))
        .then_some(AiProxyFailureClass::BackendRequestTimeout),
        ErrorType::InvalidHTTPHeader
        | ErrorType::H1Error
        | ErrorType::H2Error
        | ErrorType::H2Downgrade
        | ErrorType::InvalidH2
        | ErrorType::ReadError
        | ErrorType::WriteError
        | ErrorType::ConnectionClosed => Some(AiProxyFailureClass::AmbiguousUpstream),
        _ => None,
    }
}

#[inline]
pub fn error_while_proxy(
    edgion_http: &EdgionHttpProxy,
    peer: &HttpPeer,
    session: &mut Session,
    e: Box<Error>,
    ctx: &mut EdgionHttpContext,
    client_reused: bool,
) -> Box<Error> {
    // filter-04b: the sentinel error from a written local reply is intentional,
    // not an upstream failure. Skip body-size accounting and outlier-detection
    // failure recording, and leave the error non-retryable.
    if ctx.local_reply_written {
        return e.more_context(format!("Peer: {}", peer));
    }

    if let Some(upstream) = ctx.get_current_upstream_mut() {
        upstream.set_response_body_size(session.upstream_body_bytes_received());
    }
    let mut e = e.more_context(format!("Peer: {}", peer));

    // Record only transport/proxy failures as local-origin. HTTPStatus errors
    // are control flow for an already-observed upstream response (status retry
    // or retry-budget denial), or a locally generated response status; the
    // response filter has already classified any real upstream status.
    if should_record_local_origin_failure(ctx.retry_on_status, e.etype()) {
        if let Some(upstream) = ctx.get_current_upstream() {
            if let (Some(service_key), Some(addr)) = (&upstream.service_key, &upstream.lb_backend_addr) {
                // Mid-proxy read/write error or transport reset — local-origin failure.
                crate::backends::get_outlier_detection_store().record_failure_by_addr(
                    service_key,
                    addr,
                    crate::backends::FailureClass::LocalOrigin,
                );
            }
        }
    }

    // With streamed body handlers installed no retry buffer was ever
    // allocated, so `retry_buffer_truncated()` reports a misleading `false`
    // ("nothing truncated") — consult the monotonic latch directly (contract
    // §4.6). The proxy also suppresses such retries on its side; this keeps
    // the local decision (and its logging) consistent.
    let buffer_ok = !session.as_ref().retry_buffer_truncated() && !ctx.request_body_stream_active;

    if ctx.has_ai_attempt_plan() {
        // A status retry was decided and transitioned exactly once in the
        // response-header hook. This callback only asks Pingora to re-enter
        // upstream_peer; it must not touch scheduler or budget state again.
        if ctx.retry_on_status && buffer_ok {
            ctx.retry_on_status = false;
            e.set_retry(true);
            return e;
        }

        let failure_class = classify_ai_proxy_failure(e.etype(), e.esource(), ctx.peer_timeout_source);
        let retry_allow_non_idempotent = ctx.retry_allow_non_idempotent();
        let common_retry_gates = buffer_ok
            && !ctx.request_timeout_exceeded()
            && !ctx.grpc_deadline_exceeded()
            && !ctx.response_write_started()
            && !has_final_response_written(session);
        let Some(failure_class) = failure_class else {
            return e;
        };
        let wants_retry = ai_failure_wants_retry(failure_class, ctx.retry_configured(), retry_allow_non_idempotent);
        let gates_allow_retry = common_retry_gates
            && (failure_class == AiProxyFailureClass::BackendRequestTimeout || retry_allow_non_idempotent);
        let decision = crate::ai::dispatch::handle_ai_transport_failure(
            ctx,
            wants_retry,
            gates_allow_retry,
            failure_class == AiProxyFailureClass::AmbiguousUpstream,
        );
        if let Some(state) = ctx.ai_state_mut() {
            state.attempt_runtime.finish_failure(match failure_class {
                AiProxyFailureClass::BackendRequestTimeout => {
                    crate::ai::attempt_runtime::AiAttemptFailure::BackendRequestTimeout
                }
                AiProxyFailureClass::AmbiguousUpstream => {
                    crate::ai::attempt_runtime::AiAttemptFailure::Ambiguous(crate::backends::FailureClass::LocalOrigin)
                }
            });
        }
        match decision {
            Ok(crate::ai::dispatch::AiRetryDecision::Retry) => {
                if let Some(upstream) = ctx.get_current_upstream_mut() {
                    match failure_class {
                        AiProxyFailureClass::BackendRequestTimeout => {
                            upstream.status = Some(504);
                            upstream.err.push("AiBackendRequestTimeoutRetry".to_string());
                        }
                        AiProxyFailureClass::AmbiguousUpstream => {
                            upstream.err.push("AiAmbiguousUpstreamRetry".to_string());
                        }
                    }
                }
                if failure_class == AiProxyFailureClass::BackendRequestTimeout {
                    ctx.record_error(EdgionErrorCode::BackendRequestTimeoutRetry);
                }
                e.set_retry(true);
            }
            Ok(crate::ai::dispatch::AiRetryDecision::BudgetDenied) => {
                crate::observe::metrics::record_retry_budget_denied();
                ctx.record_error(EdgionErrorCode::RetryBudgetExhausted);
                return Error::explain(ErrorType::HTTPStatus(503), "AI retry budget exhausted");
            }
            Ok(crate::ai::dispatch::AiRetryDecision::Final) | Err(_) => {}
        }
        return e;
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
    // Also requires that nothing has been written downstream yet: once response
    // headers are committed, a retry would corrupt the already-started response.
    if matches!(e.etype(), ErrorType::ReadTimedout | ErrorType::WriteTimedout)
        && matches!(e.esource(), ErrorSource::Upstream)
        && ctx.peer_timeout_source == Some(PeerTimeoutSource::BackendRequest)
        && ctx.retry_configured()
        && session.as_ref().response_written().is_none()
    {
        let max_retries = ctx
            .retry_attempts()
            .unwrap_or(edgion_http.edgion_gateway_config.spec.max_retries);
        let deadline_exceeded = ctx.request_timeout_exceeded() || ctx.grpc_deadline_exceeded();
        if buffer_ok && ctx.try_cnt <= max_retries && !deadline_exceeded {
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
            // Retry budget (retryConstraint): composes AFTER the maxRetries/
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
    let max_retries = ctx
        .retry_attempts()
        .unwrap_or(edgion_http.edgion_gateway_config.spec.max_retries);

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

    let can_retry = client_reused && buffer_ok && method_retry_ok && ctx.try_cnt <= max_retries && !deadline_exceeded;

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
mod tests {
    use super::{
        ai_failure_wants_retry, classify_ai_proxy_failure, should_record_local_origin_failure, AiProxyFailureClass,
    };
    use crate::PeerTimeoutSource;
    use pingora_core::{ErrorSource, ErrorType};

    #[test]
    fn local_origin_recording_excludes_status_control_flow() {
        assert!(!should_record_local_origin_failure(false, &ErrorType::HTTPStatus(503)));
        assert!(!should_record_local_origin_failure(true, &ErrorType::ReadError));
        assert!(should_record_local_origin_failure(false, &ErrorType::ReadError));
    }

    #[test]
    fn ai_failure_classification_excludes_control_flow_and_non_upstream_errors() {
        assert_eq!(
            classify_ai_proxy_failure(
                &ErrorType::HTTPStatus(503),
                &ErrorSource::Upstream,
                Some(PeerTimeoutSource::BackendRequest),
            ),
            None
        );
        assert_eq!(
            classify_ai_proxy_failure(&ErrorType::ReadError, &ErrorSource::Downstream, None),
            None
        );
        assert_eq!(
            classify_ai_proxy_failure(&ErrorType::ReadError, &ErrorSource::Upstream, None),
            Some(AiProxyFailureClass::AmbiguousUpstream)
        );
        assert_eq!(
            classify_ai_proxy_failure(&ErrorType::InternalError, &ErrorSource::Upstream, None),
            None
        );
        assert_eq!(
            classify_ai_proxy_failure(
                &ErrorType::ReadTimedout,
                &ErrorSource::Upstream,
                Some(PeerTimeoutSource::BackendRequest),
            ),
            Some(AiProxyFailureClass::BackendRequestTimeout)
        );
    }

    #[test]
    fn ambiguous_upstream_requires_explicit_replay_opt_in() {
        assert!(!ai_failure_wants_retry(
            AiProxyFailureClass::AmbiguousUpstream,
            true,
            false
        ));
        assert!(ai_failure_wants_retry(
            AiProxyFailureClass::AmbiguousUpstream,
            true,
            true
        ));
        assert!(ai_failure_wants_retry(
            AiProxyFailureClass::BackendRequestTimeout,
            true,
            false
        ));
        assert!(!ai_failure_wants_retry(
            AiProxyFailureClass::BackendRequestTimeout,
            false,
            true
        ));
    }
}
