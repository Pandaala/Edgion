//! Pure, bounded scheduling for AI upstream attempts.
//!
//! Backend selection remains the responsibility of the AiProxy route picker.
//! This state machine exposes its request-local exclusions to that picker,
//! rotates credentials within the selected backend, and owns the single
//! HTTPRoute dispatch budget shared by retries and failover.

pub const MAX_AI_BACKENDS: usize = 16;
pub const MAX_AI_CREDENTIALS_PER_BACKEND: usize = 16;
pub const MAX_AI_DISPATCH_ATTEMPTS: u8 = 8;
pub const MAX_AI_ROUTE_RETRIES: u8 = MAX_AI_DISPATCH_ATTEMPTS - 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackendCredentials<'a> {
    /// Zero disables a credential. Positive values participate in weighted
    /// selection among credentials not yet tried in the current round.
    pub weights: &'a [u32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttemptTarget {
    pub backend: u8,
    pub credential: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PickBackend {
    pub round: u8,
    /// Bit `n` excludes backend slot `n` from the route picker.
    pub excluded_backends: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PickCredential {
    pub round: u8,
    pub backend: u8,
    /// Bit `n` excludes credential slot `n` from this backend.
    pub excluded_credentials: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DispatchAttempt {
    /// One-based number in the shared HTTPRoute attempt budget.
    pub number: u8,
    pub target: AttemptTarget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The backend picker completed a scan without a dispatch-capable target.
    Unavailable,
    /// A retryable failure used the final permitted dispatch.
    RetryLimitReached,
    NonRetryableFailure,
    AmbiguousFailure,
    Completed,
    /// A deadline, replay, commitment, retry-budget, or physical-attempt gate
    /// denied a retry that the upstream outcome would otherwise permit.
    RetryDenied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextStep {
    PickBackend(PickBackend),
    PickCredential(PickCredential),
    Dispatch(AttemptTarget),
    AwaitResult(DispatchAttempt),
    Stop(StopReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchFailureKind {
    Retryable,
    NonRetryable,
    Ambiguous,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DispatchFailure {
    pub attempt: u8,
    pub source: AttemptTarget,
    pub kind: DispatchFailureKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchOutcome {
    RetryableFailure,
    NonRetryableFailure,
    AmbiguousFailure,
    Completed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchedulerEvent {
    BackendSelected { backend: u8 },
    BackendExhausted { backend: u8 },
    CredentialSelected { target: AttemptTarget },
    PreDispatchSkipped { target: AttemptTarget },
    PreDispatchBackendSkipped { backend: u8 },
    DispatchStarted { attempt: DispatchAttempt },
    DispatchFailed { failure: DispatchFailure },
    DispatchCompleted { attempt: DispatchAttempt },
    RoundAdvanced { previous: u8, current: u8 },
    ScanUnavailable,
    RetryDenied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transition {
    pub event: SchedulerEvent,
    pub next: NextStep,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchedulerError {
    TooManyBackends,
    TooManyCredentials { backend: u8 },
    TooManyRetries,
    InvalidBackend,
    BackendAlreadyExcluded,
    UnexpectedState,
}

#[derive(Clone, Copy)]
struct BackendState {
    credential_count: u8,
    weights: [u32; MAX_AI_CREDENTIALS_PER_BACKEND],
    tried_credentials: u16,
}

const EMPTY_BACKEND: BackendState = BackendState {
    credential_count: 0,
    weights: [0; MAX_AI_CREDENTIALS_PER_BACKEND],
    tried_credentials: 0,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    PickBackend,
    PickCredential { backend: u8 },
    Candidate(AttemptTarget),
    InFlight(DispatchAttempt),
    Stopped(StopReason),
}

/// Request-local AI attempt scheduler.
///
/// All collections have compile-time bounds. The scheduler performs only
/// integer arithmetic and contains no locks, I/O, timers, or logging.
pub struct AttemptScheduler {
    backends: [BackendState; MAX_AI_BACKENDS],
    backend_count: u8,
    max_dispatches: u8,
    dispatches: u8,
    round: u8,
    tried_backends: u16,
    round_had_replayable_dispatch: bool,
    failures: [DispatchFailure; MAX_AI_DISPATCH_ATTEMPTS as usize],
    failure_count: u8,
    phase: Phase,
}

const EMPTY_TARGET: AttemptTarget = AttemptTarget {
    backend: 0,
    credential: 0,
};
const EMPTY_FAILURE: DispatchFailure = DispatchFailure {
    attempt: 0,
    source: EMPTY_TARGET,
    kind: DispatchFailureKind::Retryable,
};

impl AttemptScheduler {
    pub fn new(backends: &[BackendCredentials<'_>], route_retries: u8) -> Result<Self, SchedulerError> {
        if backends.len() > MAX_AI_BACKENDS {
            return Err(SchedulerError::TooManyBackends);
        }
        if route_retries > MAX_AI_ROUTE_RETRIES {
            return Err(SchedulerError::TooManyRetries);
        }

        let mut compiled = [EMPTY_BACKEND; MAX_AI_BACKENDS];
        for (backend_index, backend) in backends.iter().enumerate() {
            if backend.weights.len() > MAX_AI_CREDENTIALS_PER_BACKEND {
                return Err(SchedulerError::TooManyCredentials {
                    backend: backend_index as u8,
                });
            }
            compiled[backend_index].credential_count = backend.weights.len() as u8;
            compiled[backend_index].weights[..backend.weights.len()].copy_from_slice(backend.weights);
        }

        Ok(Self {
            backends: compiled,
            backend_count: backends.len() as u8,
            max_dispatches: route_retries + 1,
            dispatches: 0,
            round: 0,
            tried_backends: 0,
            round_had_replayable_dispatch: false,
            failures: [EMPTY_FAILURE; MAX_AI_DISPATCH_ATTEMPTS as usize],
            failure_count: 0,
            phase: Phase::PickBackend,
        })
    }

    pub fn next_step(&self) -> NextStep {
        match self.phase {
            Phase::PickBackend => NextStep::PickBackend(PickBackend {
                round: self.round,
                excluded_backends: self.tried_backends,
            }),
            Phase::PickCredential { backend } => NextStep::PickCredential(PickCredential {
                round: self.round,
                backend,
                excluded_credentials: self.backends[backend as usize].tried_credentials,
            }),
            Phase::Candidate(target) => NextStep::Dispatch(target),
            Phase::InFlight(attempt) => NextStep::AwaitResult(attempt),
            Phase::Stopped(reason) => NextStep::Stop(reason),
        }
    }

    pub fn dispatch_count(&self) -> u8 {
        self.dispatches
    }

    pub fn max_dispatches(&self) -> u8 {
        self.max_dispatches
    }

    pub fn in_flight_attempt(&self) -> Option<DispatchAttempt> {
        match self.phase {
            Phase::InFlight(attempt) => Some(attempt),
            _ => None,
        }
    }

    /// Stop without recording another failure or consuming capacity.
    pub fn deny_retry(&mut self, kind: DispatchFailureKind) -> Result<Transition, SchedulerError> {
        if matches!(self.phase, Phase::Stopped(_)) {
            return Err(SchedulerError::UnexpectedState);
        }
        if let Phase::InFlight(attempt) = self.phase {
            self.record_failure(DispatchFailure {
                attempt: attempt.number,
                source: attempt.target,
                kind,
            })?;
        }
        self.phase = Phase::Stopped(StopReason::RetryDenied);
        Ok(self.transition(SchedulerEvent::RetryDenied))
    }

    pub fn round(&self) -> u8 {
        self.round
    }

    pub fn failures(&self) -> &[DispatchFailure] {
        &self.failures[..self.failure_count as usize]
    }

    /// Accept a backend chosen by the route picker from `PickBackend`.
    pub fn select_backend(&mut self, backend: u8) -> Result<Transition, SchedulerError> {
        if self.phase != Phase::PickBackend {
            return Err(SchedulerError::UnexpectedState);
        }
        if backend >= self.backend_count {
            return Err(SchedulerError::InvalidBackend);
        }
        if self.tried_backends & bit(backend) != 0 {
            return Err(SchedulerError::BackendAlreadyExcluded);
        }
        self.phase = Phase::PickCredential { backend };
        Ok(self.transition(SchedulerEvent::BackendSelected { backend }))
    }

    /// Choose one credential by weight among the caller's currently eligible
    /// slots and credentials not yet tried in this round.
    ///
    /// `entropy` is request/attempt-local input supplied by the caller; this
    /// module does not acquire an RNG lock. If no slot remains, the backend is
    /// excluded and control returns to the route picker.
    pub fn select_credential(&mut self, eligible_credentials: u16, entropy: u64) -> Result<Transition, SchedulerError> {
        let Phase::PickCredential { backend } = self.phase else {
            return Err(SchedulerError::UnexpectedState);
        };
        let state = &mut self.backends[backend as usize];
        let configured = low_bits(state.credential_count);
        let eligible = eligible_credentials & configured & !state.tried_credentials;

        let total = (0..state.credential_count).fold(0u64, |sum, credential| {
            if eligible & bit(credential) == 0 {
                sum
            } else {
                sum + u64::from(state.weights[credential as usize])
            }
        });
        if total == 0 {
            self.tried_backends |= bit(backend);
            self.phase = Phase::PickBackend;
            return Ok(self.transition(SchedulerEvent::BackendExhausted { backend }));
        }

        let mut ticket = entropy % total;
        let mut selected = None;
        for credential in 0..state.credential_count {
            if eligible & bit(credential) == 0 {
                continue;
            }
            let weight = u64::from(state.weights[credential as usize]);
            if ticket < weight {
                selected = Some(credential);
                break;
            }
            ticket -= weight;
        }
        let Some(credential) = selected else {
            return Err(SchedulerError::UnexpectedState);
        };
        state.tried_credentials |= bit(credential);
        let target = AttemptTarget { backend, credential };
        self.phase = Phase::Candidate(target);
        Ok(self.transition(SchedulerEvent::CredentialSelected { target }))
    }

    /// Reject the selected target before the dispatch boundary. This consumes
    /// no attempt, retry, retry-budget sample, or failure-ledger slot.
    pub fn skip_predispatch(&mut self) -> Result<Transition, SchedulerError> {
        let Phase::Candidate(target) = self.phase else {
            return Err(SchedulerError::UnexpectedState);
        };
        self.phase = Phase::PickCredential {
            backend: target.backend,
        };
        Ok(self.transition(SchedulerEvent::PreDispatchSkipped { target }))
    }

    /// Reject a candidate for a backend-wide predispatch reason, such as an
    /// aggregate concurrency cap. No credential in that backend is scanned.
    pub fn skip_predispatch_backend(&mut self) -> Result<Transition, SchedulerError> {
        let Phase::Candidate(target) = self.phase else {
            return Err(SchedulerError::UnexpectedState);
        };
        self.tried_backends |= bit(target.backend);
        self.phase = Phase::PickBackend;
        Ok(self.transition(SchedulerEvent::PreDispatchBackendSkipped {
            backend: target.backend,
        }))
    }

    /// Cross the dispatch boundary and allocate the next shared attempt slot.
    pub fn start_dispatch(&mut self) -> Result<Transition, SchedulerError> {
        let Phase::Candidate(target) = self.phase else {
            return Err(SchedulerError::UnexpectedState);
        };
        if self.dispatches >= self.max_dispatches || self.dispatches >= MAX_AI_DISPATCH_ATTEMPTS {
            self.phase = Phase::Stopped(StopReason::RetryLimitReached);
            return Err(SchedulerError::UnexpectedState);
        }
        self.dispatches += 1;
        let attempt = DispatchAttempt {
            number: self.dispatches,
            target,
        };
        self.phase = Phase::InFlight(attempt);
        Ok(self.transition(SchedulerEvent::DispatchStarted { attempt }))
    }

    /// Finish the current dispatched attempt and return both the accounting
    /// event and the next integration action.
    pub fn finish_dispatch(&mut self, outcome: DispatchOutcome) -> Result<Transition, SchedulerError> {
        let Phase::InFlight(attempt) = self.phase else {
            return Err(SchedulerError::UnexpectedState);
        };
        if outcome == DispatchOutcome::Completed {
            self.phase = Phase::Stopped(StopReason::Completed);
            return Ok(self.transition(SchedulerEvent::DispatchCompleted { attempt }));
        }

        let kind = match outcome {
            DispatchOutcome::RetryableFailure => DispatchFailureKind::Retryable,
            DispatchOutcome::NonRetryableFailure => DispatchFailureKind::NonRetryable,
            DispatchOutcome::AmbiguousFailure => DispatchFailureKind::Ambiguous,
            DispatchOutcome::Completed => return Err(SchedulerError::UnexpectedState),
        };
        let failure = DispatchFailure {
            attempt: attempt.number,
            source: attempt.target,
            kind,
        };
        self.record_failure(failure)?;

        self.phase = match kind {
            DispatchFailureKind::Retryable if self.dispatches < self.max_dispatches => {
                self.round_had_replayable_dispatch = true;
                Phase::PickCredential {
                    backend: attempt.target.backend,
                }
            }
            DispatchFailureKind::Retryable => Phase::Stopped(StopReason::RetryLimitReached),
            DispatchFailureKind::NonRetryable => Phase::Stopped(StopReason::NonRetryableFailure),
            DispatchFailureKind::Ambiguous => Phase::Stopped(StopReason::AmbiguousFailure),
        };
        Ok(self.transition(SchedulerEvent::DispatchFailed { failure }))
    }

    /// Record an ambiguous dispatched failure while explicitly allowing a
    /// replay. The failure keeps its ambiguity classification for accounting,
    /// but advances through the same credential/backend path as a retryable
    /// failure.
    pub fn retry_ambiguous_dispatch(&mut self) -> Result<Transition, SchedulerError> {
        let Phase::InFlight(attempt) = self.phase else {
            return Err(SchedulerError::UnexpectedState);
        };
        let failure = DispatchFailure {
            attempt: attempt.number,
            source: attempt.target,
            kind: DispatchFailureKind::Ambiguous,
        };
        self.record_failure(failure)?;
        self.phase = if self.dispatches < self.max_dispatches {
            self.round_had_replayable_dispatch = true;
            Phase::PickCredential {
                backend: attempt.target.backend,
            }
        } else {
            Phase::Stopped(StopReason::RetryLimitReached)
        };
        Ok(self.transition(SchedulerEvent::DispatchFailed { failure }))
    }

    /// Tell the scheduler that the route picker completed its current backend
    /// scan. Tried sets are cleared only after a round containing a dispatched
    /// retryable failure and while another dispatch slot remains.
    pub fn complete_backend_scan(&mut self) -> Result<Transition, SchedulerError> {
        if self.phase != Phase::PickBackend {
            return Err(SchedulerError::UnexpectedState);
        }
        if self.round_had_replayable_dispatch && self.dispatches < self.max_dispatches {
            let previous = self.round;
            self.round = self.round.saturating_add(1);
            self.tried_backends = 0;
            for backend in &mut self.backends[..self.backend_count as usize] {
                backend.tried_credentials = 0;
            }
            self.round_had_replayable_dispatch = false;
            self.phase = Phase::PickBackend;
            return Ok(self.transition(SchedulerEvent::RoundAdvanced {
                previous,
                current: self.round,
            }));
        }

        self.phase = Phase::Stopped(StopReason::Unavailable);
        Ok(self.transition(SchedulerEvent::ScanUnavailable))
    }

    fn record_failure(&mut self, failure: DispatchFailure) -> Result<(), SchedulerError> {
        let index = self.failure_count as usize;
        if index >= self.failures.len() {
            return Err(SchedulerError::UnexpectedState);
        }
        self.failures[index] = failure;
        self.failure_count += 1;
        Ok(())
    }

    fn transition(&self, event: SchedulerEvent) -> Transition {
        Transition {
            event,
            next: self.next_step(),
        }
    }
}

fn bit(slot: u8) -> u16 {
    1u16 << slot
}

fn low_bits(count: u8) -> u16 {
    if count as usize == MAX_AI_CREDENTIALS_PER_BACKEND {
        u16::MAX
    } else {
        (1u16 << count) - 1
    }
}

#[cfg(test)]
mod tests {
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
}
