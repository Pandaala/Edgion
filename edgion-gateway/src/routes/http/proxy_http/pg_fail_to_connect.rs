use super::EdgionHttpProxy;
use crate::EdgionHttpContext;
use edgion_resources::EdgionErrorCode;
use pingora_core::prelude::HttpPeer;
use pingora_core::{Error, ErrorType};
use pingora_proxy::Session;
use std::time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectRetryContinuation {
    Stop,
    SettleAiOnly,
    SettleAiAndSelectSuccessor,
    EvaluateOrdinaryBudget,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExpiredConnectDeadline {
    Request,
    Grpc,
}

fn select_expired_connect_deadline(
    request_deadline_exceeded: bool,
    grpc_deadline_exceeded: bool,
) -> Option<ExpiredConnectDeadline> {
    if request_deadline_exceeded {
        Some(ExpiredConnectDeadline::Request)
    } else if grpc_deadline_exceeded {
        Some(ExpiredConnectDeadline::Grpc)
    } else {
        None
    }
}

fn finish_expired_connect_deadline(
    ctx: &mut EdgionHttpContext,
    deadline: ExpiredConnectDeadline,
    error: Box<Error>,
) -> Box<Error> {
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        upstream.status = Some(504);
    }
    ctx.record_error(match deadline {
        ExpiredConnectDeadline::Request => EdgionErrorCode::RequestTimeoutExceeded,
        ExpiredConnectDeadline::Grpc => EdgionErrorCode::GrpcClientDeadlineExceeded,
    });

    let mut timeout_error = Error::because(
        ErrorType::HTTPStatus(504),
        "end-to-end deadline expired after upstream connect failure",
        error,
    );
    timeout_error.set_retry(false);
    timeout_error
}

fn connect_retry_continuation(is_ai: bool, relay_allows_retry: bool) -> ConnectRetryContinuation {
    match (is_ai, relay_allows_retry) {
        (true, false) => ConnectRetryContinuation::SettleAiOnly,
        (true, true) => ConnectRetryContinuation::SettleAiAndSelectSuccessor,
        (false, false) => ConnectRetryContinuation::Stop,
        (false, true) => ConnectRetryContinuation::EvaluateOrdinaryBudget,
    }
}

#[inline]
pub fn fail_to_connect(
    edgion_http: &EdgionHttpProxy,
    session: &mut Session,
    _peer: &HttpPeer,
    ctx: &mut EdgionHttpContext,
    mut e: Box<Error>,
) -> Box<Error> {
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        upstream.set_response_body_size(0);
    }
    // Set status code based on error type: 504 for timeout, 503 for other errors
    if let Some(upstream) = ctx.get_current_upstream_mut() {
        let status_code = match e.etype() {
            ErrorType::ConnectTimedout | ErrorType::TLSHandshakeTimedout => 504,
            _ => 503,
        };
        upstream.status = Some(status_code);
        // Record the failure reason for every attempt, timeouts included: a
        // silent 504 connect-timeout entry is indistinguishable from a per-try
        // timeout entry (also 504) when aggregating retries by reason.
        upstream.err.push(e.etype().as_str().to_string());
        upstream.et = Some(
            Instant::now()
                .saturating_duration_since(upstream.start_time)
                .as_millis() as u64,
        );
    }

    // Record outlier detection failure
    if let Some(upstream) = ctx.get_current_upstream() {
        if let (Some(backend), Some(token)) = (upstream.lb_backend.as_deref(), upstream.lb_outlier_completion.as_ref())
        {
            // Connect failure / connect timeout / TLS-handshake timeout — the data plane
            // never got a usable HTTP response, so this is a local-origin failure.
            crate::backends::get_outlier_detection_store().record_failure_for_backend_with_token(
                backend,
                token,
                crate::backends::FailureClass::LocalOrigin,
            );
        }
    }

    // Check request timeout dynamically before allowing retry
    // If timeout is exceeded, block all retries and return 504. Both the
    // route-level `request_timeout` and a client-supplied `grpc-timeout`
    // (which can be tighter than the route timeout) abort retries; either
    // deadline blown means a new LB pick / connection / outlier-detection
    // counter increment would only burn resources the caller has given up on.
    if let Some(deadline) =
        select_expired_connect_deadline(ctx.request_timeout_exceeded(), ctx.grpc_deadline_exceeded())
    {
        return finish_expired_connect_deadline(ctx, deadline, e);
    }

    let relay_allows_retry = session.request_relay_retry_state().can_start_next_attempt();

    let continuation = connect_retry_continuation(ctx.has_ai_attempt_plan(), relay_allows_retry);

    if matches!(
        continuation,
        ConnectRetryContinuation::SettleAiOnly | ConnectRetryContinuation::SettleAiAndSelectSuccessor
    ) {
        // Settle the failed current predispatch attempt even when the relay
        // forbids a successor; this releases its reservation promptly.
        if let Some(state) = ctx.ai_state_mut() {
            state.attempt_runtime.finish_predispatch();
        }
        // Only the successor-selection side effects are retry-owned.
        if continuation == ConnectRetryContinuation::SettleAiOnly {
            return e;
        }
        // The typed predispatch hook consumes no scheduler dispatch slot,
        // failure-ledger entry, or retry-budget numerator. The bounded Redis
        // release is drained before the next selection or at final logging.
        if crate::ai::dispatch::handle_ai_predispatch_failure(ctx, true) {
            e.set_retry(true);
        }
        return e;
    }

    // Apply the structural relay gate before ordinary retry-budget
    // consumption so a retry Pingora cannot start has no retry-owned effects.
    if continuation == ConnectRetryContinuation::Stop {
        return e;
    }

    // Determine max_retries: rule.retry.attempts > global (HTTP or gRPC route)
    let max_retries = ctx
        .retry_attempts()
        .unwrap_or(edgion_http.edgion_gateway_config.spec.max_retries);

    if ctx.try_cnt <= max_retries {
        // Retry budget (retryConstraint): composes AFTER the maxRetries/deadline
        // checks above (D3) — both gates must pass. On refusal the retry flag is
        // NOT set; instead the client gets a deterministic 503, delivered by
        // swapping the error's ErrorType (pg_fail_to_proxy renders the status from
        // the error, never from ctx.request_info.status). The per-upstream entry
        // keeps the backend's real observed status (set above); the 503 is a
        // request-level outcome carried by the error.
        if let Some(budget) = super::pg_upstream_peer::retry_budget_for(ctx) {
            if !budget.try_retry() {
                crate::observe::metrics::record_retry_budget_denied();
                ctx.record_error(EdgionErrorCode::RetryBudgetExhausted);
                return Error::explain(ErrorType::HTTPStatus(503), "retry budget exhausted");
            }
        }
        e.set_retry(true);
    }

    e
}

#[cfg(test)]
mod tests {
    use super::{
        connect_retry_continuation, finish_expired_connect_deadline, select_expired_connect_deadline,
        ConnectRetryContinuation::*, ExpiredConnectDeadline,
    };
    use crate::observe::RequestDiagnostic;
    use crate::{BackendContext, EdgionHttpContext, UpstreamInfo};
    use edgion_resources::EdgionErrorCode;
    use pingora_core::{Error, ErrorSource, ErrorType};
    use std::time::Instant;

    fn ctx_with_attempt() -> EdgionHttpContext {
        let mut ctx = EdgionHttpContext::new();
        ctx.backend_context = Some(BackendContext {
            effective_policy: None,
            source: None,
            rsc: None,
            service_key: None,
            service_kind: None,
            upstreams: vec![UpstreamInfo {
                ip: None,
                port: None,
                status: Some(503),
                ct: None,
                ht: None,
                bt: None,
                et: Some(7),
                wpt: None,
                response_body_size: Some(0),
                start_time: Instant::now(),
                err: vec!["ConnectError".to_string()],
                lb_backend_addr: None,
                lb_backend: None,
                lb_outlier_completion: None,
                service_key: None,
                lb_policy: None,
                tls: None,
                lb_committed: None,
            }],
            current_upstream_id: Some(0),
            backend_ref_idx: None,
        });
        ctx
    }

    fn assert_deadline_outcome(deadline: ExpiredConnectDeadline, expected_diagnostic: EdgionErrorCode) {
        let mut ctx = ctx_with_attempt();
        let mut original = Error::new_up(ErrorType::ConnectError);
        original.set_retry(true);

        let outcome = finish_expired_connect_deadline(&mut ctx, deadline, original);

        assert_eq!(outcome.etype(), &ErrorType::HTTPStatus(504));
        assert!(!outcome.retry());
        assert_eq!(
            ctx.diagnostics.entries(),
            &[RequestDiagnostic::Error(expected_diagnostic)]
        );

        let upstream = ctx.get_current_upstream().unwrap();
        assert_eq!(upstream.status, Some(504));
        assert_eq!(upstream.et, Some(7));
        assert_eq!(upstream.err, ["ConnectError"]);

        let cause = outcome
            .cause
            .as_deref()
            .and_then(|cause| cause.downcast_ref::<Box<Error>>())
            .expect("deadline outcome must retain the original connect error");
        assert_eq!(cause.etype(), &ErrorType::ConnectError);
        assert_eq!(cause.esource(), &ErrorSource::Upstream);
        assert!(cause.retry());
    }

    #[test]
    fn disabled_relay_settles_ai_without_selecting_a_successor() {
        assert_eq!(connect_retry_continuation(true, false), SettleAiOnly);
        assert_eq!(connect_retry_continuation(true, true), SettleAiAndSelectSuccessor);
    }

    #[test]
    fn disabled_relay_stops_before_ordinary_retry_budget() {
        assert_eq!(connect_retry_continuation(false, false), Stop);
        assert_eq!(connect_retry_continuation(false, true), EvaluateOrdinaryBudget);
    }

    #[test]
    fn expired_deadline_selection_prefers_request() {
        assert_eq!(select_expired_connect_deadline(false, false), None);
        assert_eq!(
            select_expired_connect_deadline(true, false),
            Some(ExpiredConnectDeadline::Request)
        );
        assert_eq!(
            select_expired_connect_deadline(false, true),
            Some(ExpiredConnectDeadline::Grpc)
        );
        assert_eq!(
            select_expired_connect_deadline(true, true),
            Some(ExpiredConnectDeadline::Request)
        );
    }

    #[test]
    fn request_deadline_returns_non_retryable_504_with_original_cause() {
        assert_deadline_outcome(ExpiredConnectDeadline::Request, EdgionErrorCode::RequestTimeoutExceeded);
    }

    #[test]
    fn grpc_deadline_returns_non_retryable_504_with_original_cause() {
        assert_deadline_outcome(
            ExpiredConnectDeadline::Grpc,
            EdgionErrorCode::GrpcClientDeadlineExceeded,
        );
    }
}
