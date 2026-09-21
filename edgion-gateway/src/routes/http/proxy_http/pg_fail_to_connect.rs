use super::EdgionHttpProxy;
use crate::ai::dispatch::AiPeerPredispatchFailure;
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

fn connect_timeout_deadline(
    error: &ErrorType,
    source: Option<crate::ctx::ConnectTimeoutSource>,
    request_remaining: Option<std::time::Duration>,
    grpc_remaining: Option<std::time::Duration>,
) -> Option<ExpiredConnectDeadline> {
    use crate::ctx::ConnectTimeoutSource;
    // Pingora's timer resolution can fire just before Instant reaches the
    // deadline. The OS also maps ETIMEDOUT to ConnectTimedout, so the configured
    // timer owner is insufficient: its budget must be within the timer's
    // sub-millisecond early-firing window. Other failures remain ordinary errors.
    if !matches!(error, ErrorType::ConnectTimedout | ErrorType::TLSHandshakeTimedout) {
        return None;
    }
    let (remaining, deadline) = match source {
        Some(ConnectTimeoutSource::RequestDeadline) => (request_remaining?, ExpiredConnectDeadline::Request),
        Some(ConnectTimeoutSource::GrpcDeadline) => (grpc_remaining?, ExpiredConnectDeadline::Grpc),
        Some(ConnectTimeoutSource::Configured) | None => return None,
    };
    (remaining <= std::time::Duration::from_millis(1)).then_some(deadline)
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
        "end-to-end deadline exhausted during upstream connection establishment",
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

fn finish_ai_connect_failure(ctx: &mut EdgionHttpContext, continuation: ConnectRetryContinuation, error: &mut Error) {
    match continuation {
        ConnectRetryContinuation::SettleAiOnly => {
            if let Some(state) = ctx.ai_state_mut() {
                state.attempt_runtime.finish_predispatch();
            }
        }
        ConnectRetryContinuation::SettleAiAndSelectSuccessor => {
            if AiPeerPredispatchFailure::Endpoint.finish(ctx) {
                error.set_retry(true);
            }
        }
        ConnectRetryContinuation::Stop | ConnectRetryContinuation::EvaluateOrdinaryBudget => {}
    }
}

#[inline]
pub fn fail_to_connect(
    _edgion_http: &EdgionHttpProxy,
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
    let now = Instant::now();
    let request_remaining = ctx
        .request_timeout()
        .map(|budget| budget.saturating_sub(now.saturating_duration_since(ctx.start_time)));
    let grpc_remaining = ctx
        .request_info
        .grpc_client_deadline()
        .map(|deadline| deadline.saturating_duration_since(now));
    if let Some(deadline) = select_expired_connect_deadline(
        request_remaining.is_some_and(|remaining| remaining.is_zero()),
        grpc_remaining.is_some_and(|remaining| remaining.is_zero()),
    )
    .or_else(|| connect_timeout_deadline(e.etype(), ctx.connect_timeout_source, request_remaining, grpc_remaining))
    {
        return finish_expired_connect_deadline(ctx, deadline, e);
    }

    let relay_allows_retry = session.request_relay_retry_state().can_start_next_attempt();

    let continuation = connect_retry_continuation(ctx.has_ai_attempt_plan(), relay_allows_retry);

    if matches!(
        continuation,
        ConnectRetryContinuation::SettleAiOnly | ConnectRetryContinuation::SettleAiAndSelectSuccessor
    ) {
        // A relay veto releases only the current reservation. When a retry is
        // structurally possible, the typed endpoint failure owns both that
        // release and service-wide scheduler advancement. Neither path consumes
        // a dispatch slot, failure-ledger entry, or retry-budget numerator.
        finish_ai_connect_failure(ctx, continuation, &mut e);
        return e;
    }

    // Apply the structural relay gate before ordinary retry-budget
    // consumption so a retry Pingora cannot start has no retry-owned effects.
    if continuation == ConnectRetryContinuation::Stop {
        return e;
    }

    // Determine retry attempts: rule.retry.attempts > global (HTTP or gRPC route)
    let retry_attempts = ctx.retry_attempts().unwrap_or(ctx.egc.retry_attempts);

    if super::retry_budget_allows_next_attempt(ctx.try_cnt, retry_attempts) {
        // Retry budget (retryConstraint): composes AFTER the attempts/deadline
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
        connect_retry_continuation, finish_ai_connect_failure, finish_expired_connect_deadline,
        select_expired_connect_deadline, ConnectRetryContinuation::*, ExpiredConnectDeadline,
    };
    use crate::ai::attempt::{AttemptScheduler, BackendCredentials, NextStep, SchedulerError};
    use crate::ai::{AiAttemptPlan, AiBackendStore, ReadyAiAttemptPlan};
    use crate::observe::RequestDiagnostic;
    use crate::{BackendContext, EdgionHttpContext, UpstreamInfo};
    use bytes::Bytes;
    use edgion_resources::resources::edgion_plugins::AiBackendSelection;
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

    fn ctx_with_selected_ai_target() -> EdgionHttpContext {
        let failed_service_weights = [1, 1];
        let successor_service_weights = [1];
        let mut scheduler = AttemptScheduler::new(
            &[
                BackendCredentials {
                    weights: &failed_service_weights,
                },
                BackendCredentials {
                    weights: &successor_service_weights,
                },
            ],
            0,
        )
        .unwrap();
        scheduler.select_backend(0).unwrap();
        scheduler.select_credential(u16::MAX, 0).unwrap();

        let plan = ReadyAiAttemptPlan {
            canonical_openai_body: Bytes::from_static(br#"{"model":"logical"}"#),
            canonical_path: "/v1/chat/completions".to_string(),
            canonical_query: None,
            logical_model: "logical".to_string(),
            requested_streaming: false,
            inject_usage_tracking: false,
            input_chars: 19,
            selection: AiBackendSelection::Weighted,
            selection_sequence: 0,
            candidates: Vec::new(),
            scheduler,
            retry_cap_initialized: true,
            backend_snapshot: AiBackendStore::new().snapshot(),
            quota: None,
            quota_status: None,
            terminate_on_over_limit: false,
        };
        let mut ctx = EdgionHttpContext::new();
        ctx.restore_ai_attempt_plan(AiAttemptPlan::Ready(plan));
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

        let mut ctx = ctx_with_selected_ai_target();
        let mut error = Error::new_up(ErrorType::ConnectError);
        finish_ai_connect_failure(&mut ctx, SettleAiOnly, &mut error);

        assert!(!error.retry());
        let Some(AiAttemptPlan::Ready(ready)) = ctx.ai_state().and_then(|state| state.attempt_plan.as_ref()) else {
            panic!("ready plan must remain installed");
        };
        assert_eq!(
            ready.scheduler.next_step(),
            NextStep::Dispatch(crate::ai::attempt::AttemptTarget {
                backend: 0,
                credential: 0,
            })
        );
        assert_eq!(ready.scheduler.dispatch_count(), 0);
        assert!(ready.scheduler.failures().is_empty());
    }

    #[test]
    fn retryable_ai_connect_failure_skips_the_failed_service() {
        let mut ctx = ctx_with_selected_ai_target();
        let mut error = Error::new_up(ErrorType::ConnectError);

        finish_ai_connect_failure(&mut ctx, SettleAiAndSelectSuccessor, &mut error);

        assert!(error.retry());
        let Some(AiAttemptPlan::Ready(ready)) = ctx.ai_state_mut().and_then(|state| state.attempt_plan.as_mut()) else {
            panic!("ready plan must remain installed");
        };
        assert_eq!(
            ready.scheduler.next_step(),
            NextStep::PickBackend(crate::ai::attempt::PickBackend {
                round: 0,
                excluded_backends: 1,
            }),
            "the second credential must not be scanned after an endpoint failure"
        );
        assert_eq!(
            ready.scheduler.select_backend(0),
            Err(SchedulerError::BackendAlreadyExcluded)
        );
        assert!(ready.scheduler.select_backend(1).is_ok());
        assert_eq!(ready.scheduler.dispatch_count(), 0);
        assert!(ready.scheduler.failures().is_empty());
    }

    #[test]
    fn ai_connect_failure_without_scheduler_advancement_is_not_retryable() {
        let mut ctx = EdgionHttpContext::new();
        let mut error = Error::new_up(ErrorType::ConnectError);

        finish_ai_connect_failure(&mut ctx, SettleAiAndSelectSuccessor, &mut error);

        assert!(!error.retry());
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
    fn only_deadline_owned_timeout_errors_use_timer_attribution() {
        use crate::ctx::ConnectTimeoutSource::{Configured, GrpcDeadline, RequestDeadline};
        let near = Some(std::time::Duration::from_micros(500));
        for error in [ErrorType::ConnectTimedout, ErrorType::TLSHandshakeTimedout] {
            assert_eq!(super::connect_timeout_deadline(&error, None, near, near), None);
            assert_eq!(
                super::connect_timeout_deadline(&error, Some(Configured), near, near),
                None
            );
            assert_eq!(
                super::connect_timeout_deadline(&error, Some(RequestDeadline), near, near),
                Some(ExpiredConnectDeadline::Request)
            );
            assert_eq!(
                super::connect_timeout_deadline(&error, Some(GrpcDeadline), near, near),
                Some(ExpiredConnectDeadline::Grpc)
            );
        }
        for source in [None, Some(Configured), Some(RequestDeadline), Some(GrpcDeadline)] {
            assert_eq!(
                super::connect_timeout_deadline(&ErrorType::ConnectError, source, near, near),
                None
            );
            assert_eq!(
                super::connect_timeout_deadline(&ErrorType::TLSHandshakeFailure, source, near, near),
                None
            );
        }
        // An actually expired request wins even if the timer was gRPC-owned.
        assert_eq!(
            select_expired_connect_deadline(true, true).or_else(|| super::connect_timeout_deadline(
                &ErrorType::ConnectTimedout,
                Some(GrpcDeadline),
                near,
                near
            )),
            Some(ExpiredConnectDeadline::Request)
        );
    }

    #[test]
    fn os_connect_timeout_with_remaining_budget_keeps_ordinary_retry_policy() {
        use crate::ctx::ConnectTimeoutSource::{GrpcDeadline, RequestDeadline};
        // Pingora maps OS ETIMEDOUT to ConnectTimedout too. Neither a long
        // remaining budget nor an absent deadline proves its timer expired.
        for remaining in [
            None,
            Some(std::time::Duration::from_secs(30)),
            Some(std::time::Duration::from_micros(1001)),
        ] {
            for source in [RequestDeadline, GrpcDeadline] {
                assert_eq!(
                    super::connect_timeout_deadline(&ErrorType::ConnectTimedout, Some(source), remaining, remaining),
                    None
                );
            }
        }
        let near = Some(std::time::Duration::from_millis(1));
        let far = Some(std::time::Duration::from_secs(30));
        assert_eq!(
            super::connect_timeout_deadline(&ErrorType::ConnectTimedout, Some(RequestDeadline), far, near),
            None
        );
        assert_eq!(
            super::connect_timeout_deadline(&ErrorType::ConnectTimedout, Some(GrpcDeadline), near, far),
            None
        );
        assert_eq!(
            super::connect_timeout_deadline(&ErrorType::ConnectTimedout, Some(RequestDeadline), near, far),
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
