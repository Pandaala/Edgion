use super::{finish_expired_connect_deadline, select_expired_connect_deadline, ExpiredConnectDeadline};
use crate::ai::http::attempt::on_connect_failure;
use crate::ai::runtime::attempt::{AttemptScheduler, BackendCredentials, NextStep, SchedulerError};
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
        effective_policy_captured: false,
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
        api: edgion_resources::resources::edgion_plugins::AiApi::OpenaiChat,
        operation: crate::ai::runtime::attempt_plan::AiOperation::ChatCompletions,
        conversion: None,
        usage_observation: edgion_resources::resources::edgion_plugins::AiUsageObservation::Provider,
        observation_unavailable: false,
        body_generation: None,
        body_is_current: false,
        continuation: false,
        request_max_output_tokens: None,
        request_body: Bytes::from_static(br#"{"model":"logical"}"#),
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
    let mut ctx = ctx_with_selected_ai_target();
    let mut error = Error::new_up(ErrorType::ConnectError);
    on_connect_failure(&mut ctx, false, &mut error);

    assert!(!error.retry());
    let Some(AiAttemptPlan::Ready(ready)) = ctx.ai_state().and_then(|state| state.attempt_plan.as_ref()) else {
        panic!("ready plan must remain installed");
    };
    assert_eq!(
        ready.scheduler.next_step(),
        NextStep::Dispatch(crate::ai::runtime::attempt::AttemptTarget {
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

    on_connect_failure(&mut ctx, true, &mut error);

    assert!(error.retry());
    let Some(AiAttemptPlan::Ready(ready)) = ctx.ai_state_mut().and_then(|state| state.attempt_plan.as_mut()) else {
        panic!("ready plan must remain installed");
    };
    assert_eq!(
        ready.scheduler.next_step(),
        NextStep::PickBackend(crate::ai::runtime::attempt::PickBackend {
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

    on_connect_failure(&mut ctx, true, &mut error);

    assert!(!error.retry());
}

#[test]
fn disabled_relay_stops_before_ordinary_retry_budget() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            use tokio::io::AsyncWriteExt;
            let (mut client, server) = tokio::io::duplex(4096);
            client.write_all(b"GET / HTTP/1.1\r\nHost: test\r\n\r\n").await.unwrap();
            let stream: pingora_core::protocols::Stream = Box::new(server);
            let mut session = pingora_proxy::Session::new_h1(stream);
            assert!(session.read_request().await.unwrap());
            assert!(!session.request_relay_retry_state().can_start_next_attempt());
            let mut ctx = ctx_with_attempt();
            let proxy = crate::ai::http::attempt::tests::proxy_fixture();
            let peer = pingora_core::prelude::HttpPeer::new("127.0.0.1:30001", false, String::new());
            let error = super::fail_to_connect(
                &proxy,
                &mut session,
                &peer,
                &mut ctx,
                Error::new_up(ErrorType::ConnectError),
            );
            assert!(!error.retry());
            assert!(ctx.ai_state().is_none());
            assert!(!ctx.diagnostics.contains_error(EdgionErrorCode::RetryBudgetExhausted));
        });
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
