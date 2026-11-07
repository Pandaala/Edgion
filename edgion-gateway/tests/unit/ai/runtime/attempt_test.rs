use super::*;

fn scheduler(weights: &[&[u32]], retries: u8) -> AttemptScheduler {
    AttemptScheduler::new(weights.iter().map(|weights| weights.iter().copied()), retries).unwrap()
}

fn choose(scheduler: &mut AttemptScheduler, backend: u8, eligible: u16, entropy: u64) -> AttemptTarget {
    scheduler.select_backend(backend).unwrap();
    let transition = scheduler.select_credential(eligible, entropy).unwrap();
    let SchedulerEvent::CredentialSelected { target } = transition.event else {
        panic!("expected a credential selection");
    };
    target
}

fn dispatch_failure(scheduler: &mut AttemptScheduler, outcome: DispatchOutcome) -> Transition {
    scheduler.start_dispatch().unwrap();
    scheduler.finish_dispatch(outcome).unwrap()
}

#[test]
fn single_backend_retries_in_a_new_round() {
    let mut scheduler = scheduler(&[&[1]], 1);
    assert_eq!(
        choose(&mut scheduler, 0, u16::MAX, 0),
        AttemptTarget {
            backend: 0,
            credential: 0
        }
    );
    let failed = dispatch_failure(
        &mut scheduler,
        DispatchOutcome::RetryableFailure(FailureScope::Credential),
    );
    assert!(matches!(failed.next, NextStep::PickCredential(_)));

    let exhausted = scheduler.select_credential(u16::MAX, 0).unwrap();
    assert_eq!(exhausted.event, SchedulerEvent::BackendExhausted { backend: 0 });
    let advanced = scheduler.complete_backend_scan().unwrap();
    assert_eq!(
        advanced.event,
        SchedulerEvent::RoundAdvanced {
            previous: 0,
            current: 1
        }
    );

    choose(&mut scheduler, 0, u16::MAX, 0);
    scheduler.start_dispatch().unwrap();
    let completed = scheduler.finish_dispatch(DispatchOutcome::Completed).unwrap();
    assert_eq!(completed.next, NextStep::Stop(StopReason::Completed));
    assert_eq!(scheduler.dispatch_count(), 2);
    assert_eq!(
        scheduler.failures()[0].source,
        AttemptTarget {
            backend: 0,
            credential: 0
        }
    );
}

#[test]
fn rotates_weighted_credentials_before_reselecting_backend() {
    let mut scheduler = scheduler(&[&[1, 9], &[1]], 3);
    scheduler.select_backend(0).unwrap();
    let selected = scheduler.select_credential(u16::MAX, 5).unwrap();
    assert_eq!(
        selected.next,
        NextStep::Dispatch(AttemptTarget {
            backend: 0,
            credential: 1
        })
    );
    dispatch_failure(
        &mut scheduler,
        DispatchOutcome::RetryableFailure(FailureScope::Credential),
    );

    let selected = scheduler.select_credential(u16::MAX, 5).unwrap();
    assert_eq!(
        selected.next,
        NextStep::Dispatch(AttemptTarget {
            backend: 0,
            credential: 0
        })
    );
    dispatch_failure(
        &mut scheduler,
        DispatchOutcome::RetryableFailure(FailureScope::Credential),
    );

    let exhausted = scheduler.select_credential(u16::MAX, 0).unwrap();
    assert_eq!(exhausted.event, SchedulerEvent::BackendExhausted { backend: 0 });
    assert_eq!(
        exhausted.next,
        NextStep::PickBackend(PickBackend {
            round: 0,
            excluded_backends: 1,
        })
    );
    assert_eq!(
        choose(&mut scheduler, 1, u16::MAX, 0),
        AttemptTarget {
            backend: 1,
            credential: 0
        }
    );
}

#[test]
fn failure_scope_classifies_credential_statuses() {
    for status in [401, 403, 429] {
        assert_eq!(
            FailureScope::from_http_status(status),
            FailureScope::Credential,
            "{status}"
        );
    }
    for status in [408, 500, 502, 503, 504, 529] {
        assert_eq!(
            FailureScope::from_http_status(status),
            FailureScope::Endpoint,
            "{status}"
        );
    }
}

#[test]
fn endpoint_failure_fails_over_without_scanning_sibling_credentials() {
    let mut scheduler = scheduler(&[&[1, 1, 1, 1], &[1]], 2);
    choose(&mut scheduler, 0, u16::MAX, 0);
    let failed = dispatch_failure(
        &mut scheduler,
        DispatchOutcome::RetryableFailure(FailureScope::Endpoint),
    );
    assert_eq!(
        failed.next,
        NextStep::PickBackend(PickBackend {
            round: 0,
            excluded_backends: 1,
        })
    );
    assert_eq!(scheduler.select_backend(0), Err(SchedulerError::BackendAlreadyExcluded));
    assert_eq!(
        choose(&mut scheduler, 1, u16::MAX, 0),
        AttemptTarget {
            backend: 1,
            credential: 0
        }
    );
    scheduler.start_dispatch().unwrap();
    let completed = scheduler.finish_dispatch(DispatchOutcome::Completed).unwrap();
    assert_eq!(completed.next, NextStep::Stop(StopReason::Completed));
    assert_eq!(scheduler.dispatch_count(), 2);
}

#[test]
fn endpoint_failure_on_single_backend_retries_in_a_new_round() {
    let mut scheduler = scheduler(&[&[1, 1]], 1);
    choose(&mut scheduler, 0, u16::MAX, 0);
    dispatch_failure(
        &mut scheduler,
        DispatchOutcome::RetryableFailure(FailureScope::Endpoint),
    );
    let advanced = scheduler.complete_backend_scan().unwrap();
    assert_eq!(
        advanced.event,
        SchedulerEvent::RoundAdvanced {
            previous: 0,
            current: 1
        }
    );
    choose(&mut scheduler, 0, u16::MAX, 0);
    scheduler.start_dispatch().unwrap();
    assert_eq!(
        scheduler
            .finish_dispatch(DispatchOutcome::RetryableFailure(FailureScope::Endpoint))
            .unwrap()
            .next,
        NextStep::Stop(StopReason::RetryLimitReached)
    );
}

#[test]
fn predispatch_only_empty_round_stops_as_unavailable() {
    let mut scheduler = scheduler(&[&[1]], 7);
    choose(&mut scheduler, 0, u16::MAX, 0);
    let skipped = scheduler.skip_predispatch().unwrap();
    assert!(matches!(skipped.next, NextStep::PickCredential(_)));
    assert_eq!(scheduler.dispatch_count(), 0);
    assert!(scheduler.failures().is_empty());

    scheduler.select_credential(u16::MAX, 0).unwrap();
    let stopped = scheduler.complete_backend_scan().unwrap();
    assert_eq!(stopped.event, SchedulerEvent::ScanUnavailable);
    assert_eq!(stopped.next, NextStep::Stop(StopReason::Unavailable));
    assert_eq!(scheduler.round(), 0);
}

#[test]
fn backend_wide_predispatch_skip_avoids_scanning_its_credentials() {
    let mut scheduler = scheduler(&[&[1, 1], &[1]], 0);
    choose(&mut scheduler, 0, u16::MAX, 0);
    let skipped = scheduler.skip_predispatch_backend().unwrap();
    assert_eq!(skipped.event, SchedulerEvent::PreDispatchBackendSkipped { backend: 0 });
    assert_eq!(
        skipped.next,
        NextStep::PickBackend(PickBackend {
            round: 0,
            excluded_backends: 1,
        })
    );
    assert_eq!(scheduler.dispatch_count(), 0);
    assert!(scheduler.failures().is_empty());
}

#[test]
fn seven_retries_allow_exactly_eight_dispatches() {
    let mut scheduler = scheduler(&[&[1]], 7);
    for expected_attempt in 1..=8 {
        choose(&mut scheduler, 0, u16::MAX, 0);
        scheduler.start_dispatch().unwrap();
        let failed = scheduler
            .finish_dispatch(DispatchOutcome::RetryableFailure(FailureScope::Credential))
            .unwrap();
        assert_eq!(scheduler.dispatch_count(), expected_attempt);
        if expected_attempt == 8 {
            assert_eq!(failed.next, NextStep::Stop(StopReason::RetryLimitReached));
            break;
        }
        scheduler.select_credential(u16::MAX, 0).unwrap();
        scheduler.complete_backend_scan().unwrap();
    }
    assert_eq!(scheduler.failures().len(), 8);
}

#[test]
fn ambiguous_and_nonretryable_failures_stop_with_their_source() {
    for (outcome, reason, kind) in [
        (
            DispatchOutcome::AmbiguousFailure,
            StopReason::AmbiguousFailure,
            DispatchFailureKind::Ambiguous,
        ),
        (
            DispatchOutcome::NonRetryableFailure,
            StopReason::NonRetryableFailure,
            DispatchFailureKind::NonRetryable,
        ),
    ] {
        let mut scheduler = scheduler(&[&[1], &[1]], 7);
        let target = choose(&mut scheduler, 1, u16::MAX, 0);
        let stopped = dispatch_failure(&mut scheduler, outcome);
        assert_eq!(stopped.next, NextStep::Stop(reason));
        assert_eq!(
            scheduler.failures(),
            &[DispatchFailure {
                attempt: 1,
                source: target,
                kind,
            }]
        );
    }
}

#[test]
fn explicitly_replayed_ambiguity_keeps_ambiguous_attribution() {
    let mut scheduler = scheduler(&[&[1, 1]], 1);
    let source = choose(&mut scheduler, 0, u16::MAX, 0);
    scheduler.start_dispatch().unwrap();

    let transition = scheduler.retry_ambiguous_dispatch().unwrap();
    assert_eq!(
        transition.next,
        NextStep::PickBackend(PickBackend {
            round: 0,
            excluded_backends: 1,
        })
    );
    assert_eq!(
        scheduler.failures(),
        &[DispatchFailure {
            attempt: 1,
            source,
            kind: DispatchFailureKind::Ambiguous,
        }]
    );
}

#[test]
fn denied_retry_records_one_failure_with_requested_kind() {
    let mut scheduler = scheduler(&[&[1]], 1);
    let source = choose(&mut scheduler, 0, u16::MAX, 0);
    scheduler.start_dispatch().unwrap();

    let denied = scheduler.deny_retry(DispatchFailureKind::Ambiguous).unwrap();
    assert_eq!(denied.next, NextStep::Stop(StopReason::RetryDenied));
    assert_eq!(
        scheduler.failures(),
        &[DispatchFailure {
            attempt: 1,
            source,
            kind: DispatchFailureKind::Ambiguous,
        }]
    );
}

#[test]
fn bounded_configuration_is_rejected() {
    let too_many_backends = vec![&[][..]; MAX_AI_BACKENDS + 1];
    assert!(matches!(
        AttemptScheduler::new(too_many_backends.iter().map(|weights| weights.iter().copied()), 0),
        Err(SchedulerError::TooManyBackends)
    ));
    let too_many_credentials = [1; MAX_AI_CREDENTIALS_PER_BACKEND + 1];
    assert!(matches!(
        AttemptScheduler::new(
            [&too_many_credentials[..]]
                .iter()
                .map(|weights| weights.iter().copied()),
            0
        ),
        Err(SchedulerError::TooManyCredentials { backend: 0 })
    ));
    assert!(matches!(
        AttemptScheduler::new(std::iter::empty::<std::iter::Empty<u32>>(), MAX_AI_ROUTE_RETRIES + 1),
        Err(SchedulerError::TooManyRetries)
    ));
}

#[test]
fn constructor_error_precedence_is_stable() {
    let oversized = [1; MAX_AI_CREDENTIALS_PER_BACKEND + 1];
    let rows = vec![&oversized[..]; MAX_AI_BACKENDS + 1];
    assert!(matches!(
        AttemptScheduler::new(
            rows.iter().map(|weights| weights.iter().copied()),
            MAX_AI_ROUTE_RETRIES + 1
        ),
        Err(SchedulerError::TooManyBackends)
    ));
    assert!(matches!(
        AttemptScheduler::new(
            rows[..1].iter().map(|weights| weights.iter().copied()),
            MAX_AI_ROUTE_RETRIES + 1
        ),
        Err(SchedulerError::TooManyRetries)
    ));
    let rows = [&[][..], &oversized[..], &oversized[..]];
    assert!(matches!(
        AttemptScheduler::new(rows.iter().map(|weights| weights.iter().copied()), 0),
        Err(SchedulerError::TooManyCredentials { backend: 1 })
    ));
}

#[test]
fn constructor_accepts_exact_limits_and_empty_or_zero_weights() {
    let mut weights = [0; MAX_AI_CREDENTIALS_PER_BACKEND];
    weights[MAX_AI_CREDENTIALS_PER_BACKEND - 1] = u32::MAX;
    let rows = vec![&weights[..]; MAX_AI_BACKENDS];
    let mut state =
        AttemptScheduler::new(rows.iter().map(|weights| weights.iter().copied()), MAX_AI_ROUTE_RETRIES).unwrap();
    assert_eq!(state.max_dispatches(), MAX_AI_DISPATCH_ATTEMPTS);
    let target = choose(&mut state, (MAX_AI_BACKENDS - 1) as u8, u16::MAX, u64::MAX);
    assert_eq!(target.backend, (MAX_AI_BACKENDS - 1) as u8);
    assert_eq!(target.credential, (MAX_AI_CREDENTIALS_PER_BACKEND - 1) as u8);

    let mut empty = AttemptScheduler::new(std::iter::empty::<std::iter::Empty<u32>>(), 0).unwrap();
    assert!(matches!(empty.select_backend(0), Err(SchedulerError::InvalidBackend)));
    for weights in [&[][..], &[0, 0][..]] {
        let mut state = AttemptScheduler::new([weights].iter().map(|weights| weights.iter().copied()), 0).unwrap();
        state.select_backend(0).unwrap();
        assert_eq!(
            state.select_credential(u16::MAX, u64::MAX).unwrap().event,
            SchedulerEvent::BackendExhausted { backend: 0 }
        );
        assert_eq!(state.dispatch_count(), 0);
    }
}
