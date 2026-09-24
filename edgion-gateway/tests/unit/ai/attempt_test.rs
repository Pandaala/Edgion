use super::*;

fn scheduler(weights: &[&[u32]], retries: u8) -> AttemptScheduler {
    let backends: Vec<_> = weights.iter().map(|weights| BackendCredentials { weights }).collect();
    AttemptScheduler::new(&backends, retries).unwrap()
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
    let failed = dispatch_failure(&mut scheduler, DispatchOutcome::RetryableFailure);
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
    dispatch_failure(&mut scheduler, DispatchOutcome::RetryableFailure);

    let selected = scheduler.select_credential(u16::MAX, 5).unwrap();
    assert_eq!(
        selected.next,
        NextStep::Dispatch(AttemptTarget {
            backend: 0,
            credential: 0
        })
    );
    dispatch_failure(&mut scheduler, DispatchOutcome::RetryableFailure);

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
        let failed = scheduler.finish_dispatch(DispatchOutcome::RetryableFailure).unwrap();
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
    assert!(matches!(transition.next, NextStep::PickCredential(_)));
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
    let too_many_backends = vec![BackendCredentials { weights: &[] }; MAX_AI_BACKENDS + 1];
    assert!(matches!(
        AttemptScheduler::new(&too_many_backends, 0),
        Err(SchedulerError::TooManyBackends)
    ));
    let too_many_credentials = [1; MAX_AI_CREDENTIALS_PER_BACKEND + 1];
    assert!(matches!(
        AttemptScheduler::new(
            &[BackendCredentials {
                weights: &too_many_credentials,
            }],
            0
        ),
        Err(SchedulerError::TooManyCredentials { backend: 0 })
    ));
    assert!(matches!(
        AttemptScheduler::new(&[], MAX_AI_ROUTE_RETRIES + 1),
        Err(SchedulerError::TooManyRetries)
    ));
}
