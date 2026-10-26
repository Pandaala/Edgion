use super::*;

#[test]
fn owned_identity_transfer_preserves_bytes_and_capacity_bounds() {
    for (value, limit) in [
        ("", 0),
        ("short", 64),
        ("exact", 5),
        ("overlong", 3),
        ("aéz", 2),
        ("aéz", 3),
    ] {
        for spare_capacity in [0, 128] {
            let mut input = String::with_capacity(value.len() + spare_capacity);
            input.push_str(value);
            let pointer = input.as_ptr();
            let reusable = input.len() <= limit && input.capacity() == input.len();
            let expected = bounded_string(value, limit);
            let (actual, allocations) =
                crate::test_allocation_counter::count_allocations(|| into_bounded_string(input, limit));
            assert_eq!(actual, expected);
            assert_eq!(actual.capacity(), expected.capacity());
            if reusable {
                assert_eq!(actual.as_ptr(), pointer);
                assert_eq!(allocations, 0);
            }
        }
    }
}

fn closed_outlier_handle() -> AiCredentialOutlierHandle {
    outlier_handle_with_config(None)
}

fn outlier_handle_with_config(
    config: Option<edgion_resources::resources::outlier_detection::OutlierDetectionResolved>,
) -> AiCredentialOutlierHandle {
    use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};

    let store = crate::ai::runtime::credential_outlier::AiCredentialOutlierStore::new();
    let target = PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionBackend,
        namespace: "default".to_string(),
        name: "provider".to_string(),
    };
    store.configure_backend(target.clone(), config, ["primary".to_string()]);
    store
        .current_handle(&target)
        .and_then(|backend| backend.capture("primary"))
        .expect("credential handle")
}

#[tokio::test]
async fn native_off_successful_eos_restores_credential_concurrency_without_model_usage() {
    use edgion_resources::resources::edgion_plugins::AiUsageObservation;
    use edgion_resources::resources::outlier_detection::OutlierDetectionResolved;
    for config in [
        None,
        Some(OutlierDetectionResolved {
            consecutive_errors: 1,
            consecutive_gateway_errors: 1,
            consecutive_local_origin_failures: 1,
            ejection_seconds: 1,
            max_ejection_seconds: 10,
            max_ejection_percent: 100,
        }),
    ] {
        let handle = outlier_handle_with_config(config);
        assert_eq!(
            handle.apply_retry_after("1", SystemTime::UNIX_EPOCH),
            Some(Duration::from_secs(1))
        );
        assert!(matches!(handle.admit(), EjectionAdmission::Rejected));
        handle.force_expired_cooldown_for_test();
        let mut input = admission_input(Some(handle.clone()), None, OnRedisFailure::Deny);
        input.usage = Arc::new(AiUsageShared::with_reported_only(AiUsageObservation::Off));
        let shared = input.usage.clone();
        let eos = input.eos.clone();
        let mut state = AiAttemptRuntimeState::default();
        let AiAttemptAdmission::Ready(mut active) = state.admit(input).await else {
            panic!("expired credential must admit the transport probe");
        };
        assert!(matches!(handle.admit(), EjectionAdmission::Rejected));
        active.mark_dispatched();
        active.record_response(200, None);
        mark_eos(&eos);
        state.install(active);
        state.finalize(None).await;
        assert!(matches!(handle.admit(), EjectionAdmission::Closed));
        assert!(matches!(handle.admit(), EjectionAdmission::Closed));
        assert!(!shared.permits_provider_success());
        assert_eq!(
            shared.response_outcome(),
            crate::ai::protocols::openai::json_usage::ResponseOutcome::Unknown
        );
        assert_eq!(
            state.attempts()[0].usage_source,
            crate::ai::GenAiAttemptUsageSource::NotObserved
        );
        assert!(state.attempts()[0].input_tokens.is_none());
    }
}

#[tokio::test]
async fn native_off_probe_does_not_recover_without_a_valid_transport_completion() {
    use edgion_resources::resources::edgion_plugins::AiUsageObservation;
    for case in ["cancelled", "gateway_cut", "failure", "new_cooldown", "not_dispatched"] {
        let handle = closed_outlier_handle();
        handle.force_expired_cooldown_for_test();
        let mut input = admission_input(Some(handle.clone()), None, OnRedisFailure::Deny);
        input.usage = Arc::new(AiUsageShared::with_reported_only(AiUsageObservation::Off));
        let eos = input.eos.clone();
        let mut state = AiAttemptRuntimeState::default();
        let AiAttemptAdmission::Ready(mut active) = state.admit(input).await else {
            panic!("expired credential must admit the transport probe");
        };
        if case != "not_dispatched" {
            active.mark_dispatched();
        }
        active.record_response(200, None);
        if case != "cancelled" {
            mark_eos(&eos);
        }
        if case == "gateway_cut" {
            active.usage.mark_gateway_terminated();
        }
        if case == "failure" {
            active.record_failure_once(FailureClass::LocalOrigin);
        }
        if case == "new_cooldown" {
            handle.apply_cooldown(Duration::from_secs(60));
        }
        state.install(active);
        state.finalize(None).await;
        assert!(!matches!(handle.admit(), EjectionAdmission::Closed), "{case}");
    }
}

#[tokio::test]
async fn native_off_transport_success_resets_consecutive_credential_failures() {
    use crate::backends::outlier_detection::FailureTransition;
    use edgion_resources::resources::edgion_plugins::AiUsageObservation;
    use edgion_resources::resources::outlier_detection::OutlierDetectionResolved;
    let handle = outlier_handle_with_config(Some(OutlierDetectionResolved {
        consecutive_errors: 2,
        consecutive_gateway_errors: 2,
        consecutive_local_origin_failures: 2,
        ejection_seconds: 1,
        max_ejection_seconds: 10,
        max_ejection_percent: 100,
    }));
    assert!(matches!(
        handle.record_failure(FailureClass::Application),
        FailureTransition::None
    ));
    let mut input = admission_input(Some(handle.clone()), None, OnRedisFailure::Deny);
    input.usage = Arc::new(AiUsageShared::with_reported_only(AiUsageObservation::Off));
    let eos = input.eos.clone();
    let mut state = AiAttemptRuntimeState::default();
    let AiAttemptAdmission::Ready(mut active) = state.admit(input).await else {
        panic!("closed credential admission");
    };
    active.mark_dispatched();
    active.record_response(200, None);
    mark_eos(&eos);
    state.install(active);
    state.finalize(None).await;
    assert!(matches!(
        handle.record_failure(FailureClass::Application),
        FailureTransition::None
    ));
    assert!(matches!(handle.admit(), EjectionAdmission::Closed));
}

fn admission_input(
    outlier: Option<AiCredentialOutlierHandle>,
    limits: Option<AiCredentialLimits>,
    on_redis_failure: OnRedisFailure,
) -> AiAttemptAdmissionInput {
    AiAttemptAdmissionInput {
        namespace: "default".to_string(),
        backend_uid: "backend-uid".to_string(),
        credential_alias: "primary".to_string(),
        backend_name: "default/provider".to_string(),
        provider: AiProviderLabel::OpenAiCompatible,
        pricing: None,
        redis_ref: None,
        on_redis_failure,
        limits,
        estimated_input_tokens: 5,
        max_output_tokens: 10,
        usage: Arc::new(AiUsageShared::default()),
        eos: new_eos_latch(),
        outlier,
        adaptive: None,
        circuit_breaker: None,
        limiter_core: Arc::new(RedisRateLimiter::new()),
    }
}

fn attempt(input: u64, output_chars: u64, eos: bool) -> AiAttemptRuntime {
    let usage = Arc::new(AiUsageShared::default());
    usage.set_input_chars(input.saturating_mul(4));
    usage.add_output_units_for_test(output_chars);
    let latch = new_eos_latch();
    if eos {
        mark_eos(&latch);
    }
    AiAttemptRuntime {
        reservation: None,
        outlier: None,
        usage,
        eos: latch,
        estimated_input_tokens: input,
        backend_name: "default/provider".to_string(),
        provider: AiProviderLabel::OpenAiCompatible,
        credential_alias: "primary".to_string(),
        pricing: None,
        adaptive: None,
        status: None,
        retry_after_seconds: None,
        dispatched: true,
        verdict_recorded: false,
        _circuit_breaker: None,
    }
}

#[test]
fn response_success_waits_for_eos_and_transport_failure_is_single_verdict() {
    let mut current = attempt(1, 0, false);
    current.record_response(200, None);
    assert!(
        !current.verdict_recorded,
        "a response header is not a completed success"
    );
    assert!(current.record_failure_once(FailureClass::LocalOrigin));
    assert!(!current.record_failure_once(FailureClass::LocalOrigin));
}

#[test]
fn cooldown_delta_seconds_rounds_up_consistently() {
    assert_eq!(cooldown_delta_seconds(Duration::ZERO), 1);
    assert_eq!(cooldown_delta_seconds(Duration::from_millis(1_500)), 2);
    assert_eq!(cooldown_delta_seconds(Duration::from_secs(3)), 3);
}

#[test]
fn half_open_probe_is_not_completed_by_a_success_header() {
    use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
    use edgion_resources::resources::outlier_detection::OutlierDetectionResolved;

    let store = crate::ai::runtime::credential_outlier::AiCredentialOutlierStore::new();
    let target = PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionBackend,
        namespace: "default".to_string(),
        name: "provider".to_string(),
    };
    store.configure_backend(
        target.clone(),
        Some(OutlierDetectionResolved {
            consecutive_errors: 1,
            consecutive_gateway_errors: 1,
            consecutive_local_origin_failures: 1,
            ejection_seconds: 1,
            max_ejection_seconds: 10,
            max_ejection_percent: 100,
        }),
        ["primary".to_string()],
    );
    let handle = store
        .current_handle(&target)
        .and_then(|backend| backend.capture("primary"))
        .expect("credential handle");
    handle.force_expired_cooldown_for_test();
    let EjectionAdmission::Probe(probe) = handle.admit() else {
        panic!("expired credential must admit one half-open probe");
    };

    let mut current = attempt(1, 0, false);
    current.outlier = Some(OutlierLease {
        handle: handle.clone(),
        probe: Some(probe),
    });
    current.record_response(200, None);
    assert!(!current.verdict_recorded);
    assert!(current.outlier.as_ref().is_some_and(|outlier| outlier.probe.is_some()));
    assert!(current.record_failure_once(FailureClass::LocalOrigin));
    assert!(matches!(handle.admit(), EjectionAdmission::Rejected));
}

#[tokio::test]
async fn successful_eos_completes_the_half_open_probe() {
    let handle = closed_outlier_handle();
    handle.force_expired_cooldown_for_test();
    let EjectionAdmission::Probe(probe) = handle.admit() else {
        panic!("expired credential must admit one half-open probe");
    };
    let mut current = attempt(1, 0, true);
    current.status = Some(200);
    current.outlier = Some(OutlierLease {
        handle: handle.clone(),
        probe: Some(probe),
    });
    let mut state = AiAttemptRuntimeState::default();
    state.install(current);
    state.finalize(None).await;
    assert!(matches!(handle.admit(), EjectionAdmission::Closed));
}

#[tokio::test]
async fn admission_guards_active_overflow_outlier_and_redis_policy() {
    let mut missing_outlier = AiAttemptRuntimeState::default();
    assert!(matches!(
        missing_outlier
            .admit(admission_input(None, None, OnRedisFailure::Deny))
            .await,
        AiAttemptAdmission::Skip
    ));

    let handle = closed_outlier_handle();
    let mut active = AiAttemptRuntimeState::default();
    let AiAttemptAdmission::Ready(runtime) = active
        .admit(admission_input(Some(handle.clone()), None, OnRedisFailure::Deny))
        .await
    else {
        panic!("a closed credential without limits must be admitted");
    };
    active.install(runtime);
    assert!(matches!(
        active
            .admit(admission_input(Some(handle.clone()), None, OnRedisFailure::Deny))
            .await,
        AiAttemptAdmission::DependencyDenied
    ));

    let limits = Some(AiCredentialLimits {
        requests_per_minute: Some(1),
        tokens_per_minute: None,
    });
    let mut fail_open = AiAttemptRuntimeState::default();
    assert!(matches!(
        fail_open
            .admit(admission_input(
                Some(handle.clone()),
                limits.clone(),
                OnRedisFailure::Allow,
            ))
            .await,
        AiAttemptAdmission::Ready(_)
    ));
    let mut fail_closed = AiAttemptRuntimeState::default();
    assert!(matches!(
        fail_closed
            .admit(admission_input(Some(handle), limits, OnRedisFailure::Deny))
            .await,
        AiAttemptAdmission::DependencyDenied
    ));
}

#[tokio::test]
async fn failed_attempts_accumulate_input_and_final_eos_adds_output() {
    let mut state = AiAttemptRuntimeState::default();
    state.install(attempt(5, 0, false));
    state.mark_dispatched();
    state.finish_failure(AiAttemptFailure::Known(FailureClass::Gateway));

    let final_attempt = attempt(7, 9, true);
    assert_eq!(final_attempt.final_charged_tokens(), Some(10));
    state.install(final_attempt);
    state.finalize(Some(12)).await;
    assert!(state.ever_dispatched());
    assert_eq!(state.attempts().len(), 2);
    assert_eq!(state.charged_request_tokens(), Ok(15));
}

#[tokio::test]
async fn adaptive_settlement_charges_every_dispatch_but_ttft_requires_2xx_eos() {
    use std::collections::HashSet;

    use crate::ai::{AdaptiveTelemetryKey, AdaptiveTelemetryStore};
    use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};

    fn telemetry(store: &AdaptiveTelemetryStore, suffix: &str, second: u64) -> Arc<AdaptiveTelemetryState> {
        let key = AdaptiveTelemetryKey::new(
            format!("route-{suffix}"),
            "rule-a".to_string(),
            PolicyTargetKey {
                target_kind: PolicyTargetKind::EdgionBackend,
                namespace: "default".to_string(),
                name: "provider".to_string(),
            },
            "model-a".to_string(),
        )
        .unwrap();
        store.reconcile_keys(&HashSet::from([key.clone()]));
        let state = store.get(&key).unwrap();
        state.tick(second);
        state
    }

    let second = current_unix_second();
    let store = AdaptiveTelemetryStore::new();
    let success = telemetry(&store, "success", second);
    let failed_status = telemetry(&store, "failed-status", second);
    let incomplete = telemetry(&store, "incomplete", second);

    for (adaptive, status, eos) in [
        (Arc::clone(&success), 200, true),
        (Arc::clone(&failed_status), 500, true),
        (Arc::clone(&incomplete), 200, false),
    ] {
        let mut runtime = AiAttemptRuntimeState::default();
        let mut active = attempt(5, 8, eos);
        active.status = Some(status);
        active.adaptive = Some(adaptive);
        runtime.install(active);
        runtime.finalize(Some(12)).await;
        assert_eq!(runtime.charged_request_tokens(), Ok(7));
    }

    let after_settlement = current_unix_second();
    for state in [&success, &failed_status, &incomplete] {
        state.tick(after_settlement + 1);
        assert_eq!(state.snapshot().charged_tokens_60s, 7);
    }
    assert_eq!(success.snapshot().latency_samples, 1);
    assert_eq!(success.snapshot().latency_p50_micros, Some(15_000));
    assert_eq!(failed_status.snapshot().latency_samples, 0);
    assert_eq!(incomplete.snapshot().latency_samples, 0);
}

#[tokio::test]
async fn token_totals_cover_every_attempt_by_fixed_provider_and_source() {
    let mut state = AiAttemptRuntimeState::default();
    let mut failed = attempt(5, 0, false);
    failed.provider = AiProviderLabel::OpenAi;
    failed.usage.observe_reported_usage(3, 4, 0, 0);
    state.install(failed);
    state.mark_dispatched();
    state.finish_failure(AiAttemptFailure::Known(FailureClass::Gateway));

    let mut final_attempt = attempt(5, 9, true);
    final_attempt.provider = AiProviderLabel::Anthropic;
    state.install(final_attempt);
    state.mark_dispatched();
    state.finalize(None).await;

    assert_eq!(state.charged_request_tokens(), Ok(15));
    assert_eq!(
        state.metric_totals(),
        Ok([
            AiAttemptTokenTotals {
                provider: AiProviderLabel::OpenAi,
                usage_source: GenAiAttemptUsageSource::Reported,
                input_tokens: 3,
                output_tokens: 4,
            },
            AiAttemptTokenTotals {
                provider: AiProviderLabel::Anthropic,
                usage_source: GenAiAttemptUsageSource::Estimated,
                input_tokens: 5,
                output_tokens: 3,
            },
        ]
        .as_slice())
    );
}

#[tokio::test]
async fn a_gateway_terminated_stream_reconciles_its_reservation_and_is_not_an_incomplete_response() {
    use bytes::Bytes;
    use pingora_proxy::ResponseBodySink;

    use crate::ai::accounting::usage::UsageExtractionProcessor;
    use crate::ai::quota::{QuotaAccumulator, QuotaFinalStatus};
    use crate::ctx::ResponseStreamKind;
    use crate::plugins::runtime::traits::session::MockPluginSession;
    use crate::plugins::runtime::traits::streaming_response_processor::StreamingResponseProcessor;

    // Driven through a real `on_chunk` on purpose. The sibling tests build
    // their attempt state directly via `attempt(input, output_chars, eos)`,
    // which is exactly why "terminate => the eos latch is never set" stayed
    // invisible: `mark_eos` is reachable only from the `end_of_stream`
    // branch, and a terminate guarantees that branch never runs.
    let shared = Arc::new(AiUsageShared::default());
    let quota = Arc::new(QuotaAccumulator::new(16));
    quota.finish(QuotaFinalStatus::OverLimit);
    let mut processor = UsageExtractionProcessor::new_with_quota(Arc::clone(&shared), Some(quota), true, true);
    let mut session = MockPluginSession::new();
    let mut ai = crate::plugins::runtime::traits::ai_session::MockAiSessionCapability::new();
    ai.expect_semantic_guard_active().return_const(false);
    session
        .expect_ai()
        .return_var(Box::new(ai) as Box<dyn crate::plugins::runtime::traits::AiSessionCapability>);
    session.expect_try_claim_response_terminal().return_const(true);
    session.expect_mark_response_terminal_emission().return_const(());
    session.expect_record_error().return_const(());
    let mut sink = ResponseBodySink::new();
    let mut body = Some(Bytes::from_static(
        b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
    ));
    processor
        .on_chunk(&mut body, false, ResponseStreamKind::Sse, &mut sink, &mut session)
        .await;
    assert!(sink.is_terminated(), "an over-limit stream under Reject is cut here");

    let mut state = AiAttemptRuntimeState::default();
    let mut active = attempt(5, 0, false);
    active.usage = Arc::clone(&shared);
    let eos = active.eos.load(Ordering::Acquire);
    let settled = eos || active.usage.gateway_terminated();
    assert!(!eos, "the terminate above ends the response before any eos task");
    assert!(settled, "the gateway still knows this exchange is over");

    // Called directly because a unit test holds no Redis lease, so the
    // chosen operation is the only observable form of the decision.
    let operation = state.settlement_for(&mut active, settled, eos);
    assert!(
        matches!(operation, SettlementOperation::Reconcile(charged) if charged > 5),
        "the credential reservation must be reconciled down to observed consumption; \
             retaining it would hold the unconsumed remainder against every other tenant \
             sharing the credential until the rate-limit window expires"
    );

    state.install(active);
    state.mark_dispatched();
    state.finalize(None).await;
    assert_eq!(state.attempts().len(), 1);
    assert!(
        state.attempts()[0].failure.is_none(),
        "a policy-driven cut is not an upstream truncation, so it must not be logged as one"
    );
}

#[test]
fn ambiguous_failures_log_observed_input_while_redis_reservation_is_retained() {
    let mut state = AiAttemptRuntimeState::default();
    state.install(attempt(5, 100, false));
    state.mark_dispatched();
    state.finish_failure(AiAttemptFailure::Ambiguous(FailureClass::LocalOrigin));
    state.install(attempt(7, 100, false));
    state.mark_dispatched();
    state.finish_failure(AiAttemptFailure::BackendRequestTimeout);

    // Both failure branches use SettlementOperation::Retain, preserving
    // the full Redis reservation. Access and identity accounting do not
    // pretend the unobserved reserved output was consumed.
    assert_eq!(state.charged_request_tokens(), Ok(12));
    assert!(state.attempts().iter().all(|entry| entry.output_tokens == Some(0)));
}

#[tokio::test]
async fn attempt_outcomes_choose_the_expected_settlement_operation() {
    let mut predispatch = AiAttemptRuntimeState::default();
    predispatch.install(attempt(5, 0, false));
    predispatch.finish_predispatch();
    assert_eq!(
        predispatch.settlement_decisions_for_test(),
        [SettlementOperation::Release]
    );

    let mut known = AiAttemptRuntimeState::default();
    known.install(attempt(5, 0, false));
    known.finish_failure(AiAttemptFailure::Known(FailureClass::Gateway));
    assert_eq!(
        known.settlement_decisions_for_test(),
        [SettlementOperation::Reconcile(5)]
    );

    for failure in [
        AiAttemptFailure::Ambiguous(FailureClass::LocalOrigin),
        AiAttemptFailure::BackendRequestTimeout,
    ] {
        let mut state = AiAttemptRuntimeState::default();
        state.install(attempt(5, 0, false));
        state.finish_failure(failure);
        assert_eq!(state.settlement_decisions_for_test(), [SettlementOperation::Retain]);
    }

    let mut undispatched = AiAttemptRuntimeState::default();
    let mut current = attempt(5, 0, false);
    current.dispatched = false;
    undispatched.install(current);
    undispatched.finalize(None).await;
    assert_eq!(
        undispatched.settlement_decisions_for_test(),
        [SettlementOperation::Release]
    );

    let mut complete = AiAttemptRuntimeState::default();
    complete.install(attempt(5, 9, true));
    complete.finalize(None).await;
    assert_eq!(
        complete.settlement_decisions_for_test(),
        [SettlementOperation::Reconcile(8)]
    );

    let mut incomplete = AiAttemptRuntimeState::default();
    incomplete.install(attempt(5, 9, false));
    incomplete.finalize(None).await;
    assert_eq!(
        incomplete.settlement_decisions_for_test(),
        [SettlementOperation::Retain]
    );

    let mut overflow = AiAttemptRuntimeState::default();
    let overflowed = attempt(1, 0, true);
    overflowed.usage.observe_reported_usage(u64::MAX, 1, 0, 0);
    overflow.install(overflowed);
    overflow.finalize(None).await;
    assert_eq!(overflow.settlement_decisions_for_test(), [SettlementOperation::Retain]);
    assert_eq!(overflow.charged_request_tokens(), Err(()));
}

#[test]
fn settlement_decisions_map_to_redis_operations() {
    assert_eq!(
        redis_settlement_operation(SettlementOperation::Release),
        Some(CredentialReservationOperation::ReleasePredispatch)
    );
    assert_eq!(
        redis_settlement_operation(SettlementOperation::Reconcile(7)),
        Some(CredentialReservationOperation::Reconcile { charged_tokens: 7 })
    );
    assert_eq!(redis_settlement_operation(SettlementOperation::Retain), None);
}

#[test]
fn serialized_attempt_has_only_the_safe_bounded_schema() {
    let entry = GenAiAttempt {
        number: 1,
        backend: "default/backend-a".to_string(),
        provider: "openai",
        credential: "primary".to_string(),
        status: Some(429),
        failure: Some(GenAiAttemptFailure::RetryableStatus),
        retry_after_seconds: Some(3),
        ttft_ms: Some(11),
        usage_source: GenAiAttemptUsageSource::Reported,
        input_tokens: Some(2),
        output_tokens: Some(1),
        estimated_cost_usd: Some("0.00001".to_string()),
    };
    let value = serde_json::to_value(entry).unwrap();
    let keys: std::collections::BTreeSet<&str> = value.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "credential",
            "estimated_cost_usd",
            "failure",
            "input_tokens",
            "number",
            "output_tokens",
            "provider",
            "retry_after_seconds",
            "backend",
            "status",
            "ttft_ms",
            "usage_source",
        ]
        .into_iter()
        .collect()
    );
    for forbidden in ["endpoint", "ip", "secret", "body", "provider_error", "tool_args"] {
        assert!(!value.as_object().unwrap().contains_key(forbidden));
    }
}

#[test]
fn reported_usage_overflow_is_explicit() {
    let attempt = attempt(1, 0, true);
    attempt.usage.observe_reported_usage(u64::MAX, 1, 0, 0);
    assert_eq!(attempt.final_charged_tokens(), None);
}

#[tokio::test]
async fn ledger_is_bounded_to_eight_entries_and_field_bounds_fit_byte_budget() {
    let mut state = AiAttemptRuntimeState::default();
    for _ in 0..MAX_PENDING_SETTLEMENTS {
        let mut current = attempt(1, 0, false);
        current.backend_name = "s".repeat(2_000);
        current.credential_alias = "c".repeat(2_000);
        state.install(current);
        state.mark_dispatched();
        state.finish_failure(AiAttemptFailure::Ambiguous(FailureClass::LocalOrigin));
    }
    assert_eq!(state.attempts().len(), MAX_PENDING_SETTLEMENTS);
    // With eight entries and the production backend/credential field
    // bounds, the 8 KiB cap is currently defense-in-depth against future
    // schema growth rather than a separately reachable branch.
    assert!(serde_json::to_vec(state.attempts()).unwrap().len() <= MAX_AI_ATTEMPT_LOG_BYTES);
    assert!(state.attempts().iter().all(|entry| {
        entry.backend.len() <= MAX_BACKEND_KEY_BYTES && entry.credential.len() <= MAX_CREDENTIAL_ALIAS_BYTES
    }));

    state.install(attempt(1, 0, false));
    state.mark_dispatched();
    state.finish_failure(AiAttemptFailure::Ambiguous(FailureClass::LocalOrigin));
    assert_eq!(state.attempts().len(), MAX_PENDING_SETTLEMENTS);
    assert!(state.attempts_truncated());
}

#[tokio::test]
async fn accounting_overflow_fails_closed_without_wrapping() {
    let mut state = AiAttemptRuntimeState::default();
    state.install(attempt(u64::MAX, 0, false));
    state.mark_dispatched();
    state.finish_failure(AiAttemptFailure::Ambiguous(FailureClass::LocalOrigin));
    state.install(attempt(1, 0, false));
    state.mark_dispatched();
    state.finish_failure(AiAttemptFailure::Ambiguous(FailureClass::LocalOrigin));
    assert_eq!(state.charged_request_tokens(), Err(()));
    assert!(matches!(
        state
            .admit(admission_input(
                Some(closed_outlier_handle()),
                None,
                OnRedisFailure::Deny,
            ))
            .await,
        AiAttemptAdmission::DependencyDenied
    ));
}

#[tokio::test]
async fn unknown_attempts_keep_identity_and_known_metric_buckets() {
    let mut state = AiAttemptRuntimeState::default();
    state.install(attempt(2, 4, true));
    state.mark_dispatched();
    state.finalize(None).await;
    let unknown = attempt(3, 8, true);
    unknown.usage.mark_observation_unavailable();
    unknown.usage.mark_gateway_terminated();
    state.install(unknown);
    state.mark_dispatched();
    state.finalize(None).await;
    assert_eq!(state.charged_request_tokens(), Err(()));
    assert!(!state.accounting_overflow());
    assert_eq!(state.unresolved_usage_source(), Some(GenAiAttemptUsageSource::Unknown));
    assert_eq!(state.attempts().len(), 2);
    assert_eq!(state.attempts()[1].usage_source, GenAiAttemptUsageSource::Unknown);
    assert_eq!(state.attempts()[1].input_tokens, None);
    assert_eq!(state.attempts()[1].output_tokens, None);
    assert_eq!(state.attempts()[1].failure, None);
    assert_eq!(
        state.settlement_decisions_for_test(),
        [SettlementOperation::Reconcile(3), SettlementOperation::Retain]
    );
    assert_eq!(state.metric_totals().unwrap()[0].input_tokens, 2);
}

#[tokio::test]
async fn all_off_and_mixed_observation_reduce_distinctly() {
    let mut state = AiAttemptRuntimeState::default();
    let mut off = attempt(2, 0, true);
    off.usage = Arc::new(AiUsageShared::observation_disabled());
    state.install(off);
    state.mark_dispatched();
    state.finalize(None).await;
    assert_eq!(
        state.unresolved_usage_source(),
        Some(GenAiAttemptUsageSource::NotObserved)
    );
    state.install(attempt(2, 0, true));
    state.mark_dispatched();
    state.finalize(None).await;
    assert_eq!(state.unresolved_usage_source(), Some(GenAiAttemptUsageSource::Unknown));
    assert_eq!(state.attempts().len(), 2);
    assert_eq!(state.charged_request_tokens(), Err(()));
}

#[tokio::test]
async fn successful_ttft_is_recorded_without_numeric_usage() {
    use crate::ai::{AdaptiveTelemetryKey, AdaptiveTelemetryStore};
    use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
    use edgion_resources::resources::edgion_plugins::AiUsageObservation;
    let key = AdaptiveTelemetryKey::new(
        "off-route".into(),
        "rule".into(),
        PolicyTargetKey {
            target_kind: PolicyTargetKind::EdgionBackend,
            namespace: "default".into(),
            name: "provider".into(),
        },
        "logical".into(),
    )
    .unwrap();
    let store = AdaptiveTelemetryStore::new();
    store.reconcile_keys(&std::collections::HashSet::from([key.clone()]));
    let adaptive = store.get(&key).unwrap();
    adaptive.tick(current_unix_second());
    let mut active = attempt(5, 8, true);
    active.usage = Arc::new(AiUsageShared::with_observation(AiUsageObservation::Off));
    active.adaptive = Some(Arc::clone(&adaptive));
    active.status = Some(200);
    let mut state = AiAttemptRuntimeState::default();
    state.install(active);
    state.finalize(Some(12)).await;
    adaptive.tick(current_unix_second() + 1);
    assert_eq!(adaptive.snapshot().latency_samples, 1);
    assert_eq!(adaptive.snapshot().latency_p50_micros, Some(15_000));
    assert_eq!(adaptive.snapshot().charged_tokens_60s, 0);
    assert_eq!(state.attempts()[0].usage_source, GenAiAttemptUsageSource::NotObserved);
    assert!(state.attempts()[0].input_tokens.is_none());
    assert!(state.metric_totals().unwrap().is_empty());
}

#[test]
fn responses_health_verdict_requires_observed_model_completion_and_actual_eos() {
    use crate::ai::protocols::openai::json_usage::ResponseOutcome;
    use edgion_resources::resources::edgion_plugins::AiUsageObservation;
    for outcome in [
        ResponseOutcome::Completed,
        ResponseOutcome::Failed,
        ResponseOutcome::Incomplete,
        ResponseOutcome::Cancelled,
        ResponseOutcome::Unknown,
    ] {
        let mut active = attempt(3, 0, true);
        active.usage = Arc::new(AiUsageShared::with_reported_only(AiUsageObservation::Provider));
        active.usage.observe_response_outcome(outcome);
        active.usage.observe_reported_usage(11, 7, 0, 0);
        active.record_response(200, None);
        let mut state = AiAttemptRuntimeState::default();
        assert!(matches!(
            state.settlement_for(&mut active, true, true),
            SettlementOperation::Reconcile(18)
        ));
        assert_eq!(active.verdict_recorded, outcome == ResponseOutcome::Completed);
        let mut truncated = attempt(3, 0, false);
        truncated.usage = active.usage.clone();
        truncated.record_response(200, None);
        assert!(matches!(
            state.settlement_for(&mut truncated, false, false),
            SettlementOperation::Retain
        ));
        assert!(!truncated.verdict_recorded);
    }
}

#[test]
fn attempt_log_size_counts_wire_bytes_without_allocating() {
    let entries = vec![GenAiAttempt {
        number: 1,
        backend: "namespace/quoted\"\\\né".to_string(),
        provider: "openai",
        credential: "primary".to_string(),
        status: Some(429),
        failure: Some(GenAiAttemptFailure::RetryableStatus),
        retry_after_seconds: Some(3),
        ttft_ms: None,
        usage_source: GenAiAttemptUsageSource::Reported,
        input_tokens: Some(2),
        output_tokens: Some(1),
        estimated_cost_usd: Some("0.00001".to_string()),
    }];
    let encoded = serde_json::to_vec(&entries).unwrap();
    for remaining in 0..encoded.len() {
        assert!(serde_json::to_writer(AttemptLogSize { remaining }, &entries).is_err());
    }
    let mut size = AttemptLogSize {
        remaining: encoded.len(),
    };
    let (result, allocations) =
        crate::test_allocation_counter::count_allocations(|| serde_json::to_writer(&mut size, &entries));
    assert!(result.is_ok());
    assert_eq!(size.remaining, 0);
    assert_eq!(allocations, 0);
    let (_, allocations) = crate::test_allocation_counter::count_allocations(|| serde_json::to_vec(&entries).unwrap());
    assert!(allocations > 0);

    let mut state = AiAttemptRuntimeState::default();
    state.record_test_attempt(entries[0].clone(), AiProviderLabel::OpenAi);
    // Bypass production field bounds to exercise the independent log byte cap.
    let mut oversized = entries[0].clone();
    oversized.backend = "a".repeat(MAX_AI_ATTEMPT_LOG_BYTES);
    state.record_test_attempt(oversized, AiProviderLabel::OpenAi);
    assert_eq!(state.attempts(), entries);
    assert!(state.attempts_truncated());
    assert_eq!(state.charged_request_tokens(), Ok(6));
    let totals = state.metric_totals().unwrap();
    assert_eq!(totals.len(), 1);
    assert_eq!((totals[0].input_tokens, totals[0].output_tokens), (4, 2));
}

#[tokio::test]
async fn admission_breaker_permit_releases_on_rejection_drop_and_predispatch_finish() {
    use crate::ai::runtime::circuit_breaker::LogicalBackendCircuitBreaker;

    for case in 0..4 {
        let breaker = LogicalBackendCircuitBreaker::new(1);
        let mut input = admission_input(
            (case != 0).then(closed_outlier_handle),
            (case == 1).then_some(AiCredentialLimits {
                requests_per_minute: Some(1),
                tokens_per_minute: None,
            }),
            OnRedisFailure::Deny,
        );
        input.circuit_breaker = Some(Arc::clone(&breaker));
        let mut state = AiAttemptRuntimeState::default();
        match state.admit(input).await {
            AiAttemptAdmission::Skip => assert_eq!(case, 0),
            AiAttemptAdmission::DependencyDenied => assert_eq!(case, 1),
            AiAttemptAdmission::Ready(runtime) => {
                assert!(case >= 2);
                assert!(Arc::clone(&breaker).try_acquire().is_none());
                if case == 2 {
                    drop(runtime);
                } else {
                    state.install(runtime);
                    state.finish_predispatch();
                }
            }
            AiAttemptAdmission::SkipBackend => panic!("the initially empty breaker must admit"),
        }
        assert_eq!(Arc::strong_count(&breaker), 1);
        let permit = Arc::clone(&breaker)
            .try_acquire()
            .expect("the prior permit must be released");
        drop(permit);
        assert_eq!(Arc::strong_count(&breaker), 1);
    }
}

#[tokio::test]
async fn failed_query_projection_releases_undispatched_ai_reservation_at_logging_finalize() {
    let mut ctx = crate::EdgionHttpContext::new();
    let mut active = attempt(5, 0, false);
    active.dispatched = false;
    ctx.ensure_ai_state().attempt_runtime.install(active);
    ctx.request_query.capture(Some("token=secret".into())).unwrap();
    ctx.request_query.remove_current_names(&["token"]).unwrap();
    let mut header = pingora_http::RequestHeader::build("OPTIONS", b"*", None).unwrap();
    assert!(ctx.ensure_target_synced(&mut header).is_err());
    assert!(ctx.request_query.projection_is_dirty());
    for _ in 0..2 {
        crate::ai::http::finalize(&mut ctx).await;
        let state = &ctx.ai_state().unwrap().attempt_runtime;
        assert_eq!(state.settlement_decisions_for_test(), [SettlementOperation::Release]);
        assert!(state.active.is_none());
        assert!(state.attempts().is_empty());
        assert!(!state.ever_dispatched());
    }
}
