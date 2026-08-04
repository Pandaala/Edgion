//! Pure, bounded scheduling for AI upstream attempts.
//!
//! Service selection remains the responsibility of the AiProxy route picker.
//! This state machine exposes its request-local exclusions to that picker,
//! rotates credentials within the selected service, and owns the single
//! HTTPRoute dispatch budget shared by retries and failover.

pub const MAX_AI_SERVICES: usize = 16;
pub const MAX_AI_CREDENTIALS_PER_SERVICE: usize = 16;
pub const MAX_AI_DISPATCH_ATTEMPTS: u8 = 8;
pub const MAX_AI_ROUTE_RETRIES: u8 = MAX_AI_DISPATCH_ATTEMPTS - 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceCredentials<'a> {
    /// Zero disables a credential. Positive values participate in weighted
    /// selection among credentials not yet tried in the current round.
    pub weights: &'a [u32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttemptTarget {
    pub service: u8,
    pub credential: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PickService {
    pub round: u8,
    /// Bit `n` excludes service slot `n` from the route picker.
    pub excluded_services: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PickCredential {
    pub round: u8,
    pub service: u8,
    /// Bit `n` excludes credential slot `n` from this service.
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
    /// The service picker completed a scan without a dispatch-capable target.
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
    PickService(PickService),
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
    ServiceSelected { service: u8 },
    ServiceExhausted { service: u8 },
    CredentialSelected { target: AttemptTarget },
    PreDispatchSkipped { target: AttemptTarget },
    PreDispatchServiceSkipped { service: u8 },
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
    TooManyServices,
    TooManyCredentials { service: u8 },
    TooManyRetries,
    InvalidService,
    ServiceAlreadyExcluded,
    UnexpectedState,
}

#[derive(Clone, Copy)]
struct ServiceState {
    credential_count: u8,
    weights: [u32; MAX_AI_CREDENTIALS_PER_SERVICE],
    tried_credentials: u16,
}

const EMPTY_SERVICE: ServiceState = ServiceState {
    credential_count: 0,
    weights: [0; MAX_AI_CREDENTIALS_PER_SERVICE],
    tried_credentials: 0,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    PickService,
    PickCredential { service: u8 },
    Candidate(AttemptTarget),
    InFlight(DispatchAttempt),
    Stopped(StopReason),
}

/// Request-local AI attempt scheduler.
///
/// All collections have compile-time bounds. The scheduler performs only
/// integer arithmetic and contains no locks, I/O, timers, or logging.
pub struct AttemptScheduler {
    services: [ServiceState; MAX_AI_SERVICES],
    service_count: u8,
    max_dispatches: u8,
    dispatches: u8,
    round: u8,
    tried_services: u16,
    round_had_replayable_dispatch: bool,
    failures: [DispatchFailure; MAX_AI_DISPATCH_ATTEMPTS as usize],
    failure_count: u8,
    phase: Phase,
}

const EMPTY_TARGET: AttemptTarget = AttemptTarget {
    service: 0,
    credential: 0,
};
const EMPTY_FAILURE: DispatchFailure = DispatchFailure {
    attempt: 0,
    source: EMPTY_TARGET,
    kind: DispatchFailureKind::Retryable,
};

impl AttemptScheduler {
    pub fn new(services: &[ServiceCredentials<'_>], route_retries: u8) -> Result<Self, SchedulerError> {
        if services.len() > MAX_AI_SERVICES {
            return Err(SchedulerError::TooManyServices);
        }
        if route_retries > MAX_AI_ROUTE_RETRIES {
            return Err(SchedulerError::TooManyRetries);
        }

        let mut compiled = [EMPTY_SERVICE; MAX_AI_SERVICES];
        for (service_index, service) in services.iter().enumerate() {
            if service.weights.len() > MAX_AI_CREDENTIALS_PER_SERVICE {
                return Err(SchedulerError::TooManyCredentials {
                    service: service_index as u8,
                });
            }
            compiled[service_index].credential_count = service.weights.len() as u8;
            compiled[service_index].weights[..service.weights.len()].copy_from_slice(service.weights);
        }

        Ok(Self {
            services: compiled,
            service_count: services.len() as u8,
            max_dispatches: route_retries + 1,
            dispatches: 0,
            round: 0,
            tried_services: 0,
            round_had_replayable_dispatch: false,
            failures: [EMPTY_FAILURE; MAX_AI_DISPATCH_ATTEMPTS as usize],
            failure_count: 0,
            phase: Phase::PickService,
        })
    }

    pub fn next_step(&self) -> NextStep {
        match self.phase {
            Phase::PickService => NextStep::PickService(PickService {
                round: self.round,
                excluded_services: self.tried_services,
            }),
            Phase::PickCredential { service } => NextStep::PickCredential(PickCredential {
                round: self.round,
                service,
                excluded_credentials: self.services[service as usize].tried_credentials,
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

    /// Accept a service chosen by the route picker from `PickService`.
    pub fn select_service(&mut self, service: u8) -> Result<Transition, SchedulerError> {
        if self.phase != Phase::PickService {
            return Err(SchedulerError::UnexpectedState);
        }
        if service >= self.service_count {
            return Err(SchedulerError::InvalidService);
        }
        if self.tried_services & bit(service) != 0 {
            return Err(SchedulerError::ServiceAlreadyExcluded);
        }
        self.phase = Phase::PickCredential { service };
        Ok(self.transition(SchedulerEvent::ServiceSelected { service }))
    }

    /// Choose one credential by weight among the caller's currently eligible
    /// slots and credentials not yet tried in this round.
    ///
    /// `entropy` is request/attempt-local input supplied by the caller; this
    /// module does not acquire an RNG lock. If no slot remains, the service is
    /// excluded and control returns to the route picker.
    pub fn select_credential(&mut self, eligible_credentials: u16, entropy: u64) -> Result<Transition, SchedulerError> {
        let Phase::PickCredential { service } = self.phase else {
            return Err(SchedulerError::UnexpectedState);
        };
        let state = &mut self.services[service as usize];
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
            self.tried_services |= bit(service);
            self.phase = Phase::PickService;
            return Ok(self.transition(SchedulerEvent::ServiceExhausted { service }));
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
        let target = AttemptTarget { service, credential };
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
            service: target.service,
        };
        Ok(self.transition(SchedulerEvent::PreDispatchSkipped { target }))
    }

    /// Reject a candidate for a service-wide predispatch reason, such as an
    /// aggregate concurrency cap. No credential in that service is scanned.
    pub fn skip_predispatch_service(&mut self) -> Result<Transition, SchedulerError> {
        let Phase::Candidate(target) = self.phase else {
            return Err(SchedulerError::UnexpectedState);
        };
        self.tried_services |= bit(target.service);
        self.phase = Phase::PickService;
        Ok(self.transition(SchedulerEvent::PreDispatchServiceSkipped {
            service: target.service,
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
                    service: attempt.target.service,
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
    /// but advances through the same credential/service path as a retryable
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
                service: attempt.target.service,
            }
        } else {
            Phase::Stopped(StopReason::RetryLimitReached)
        };
        Ok(self.transition(SchedulerEvent::DispatchFailed { failure }))
    }

    /// Tell the scheduler that the route picker completed its current service
    /// scan. Tried sets are cleared only after a round containing a dispatched
    /// retryable failure and while another dispatch slot remains.
    pub fn complete_service_scan(&mut self) -> Result<Transition, SchedulerError> {
        if self.phase != Phase::PickService {
            return Err(SchedulerError::UnexpectedState);
        }
        if self.round_had_replayable_dispatch && self.dispatches < self.max_dispatches {
            let previous = self.round;
            self.round = self.round.saturating_add(1);
            self.tried_services = 0;
            for service in &mut self.services[..self.service_count as usize] {
                service.tried_credentials = 0;
            }
            self.round_had_replayable_dispatch = false;
            self.phase = Phase::PickService;
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
    if count as usize == MAX_AI_CREDENTIALS_PER_SERVICE {
        u16::MAX
    } else {
        (1u16 << count) - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scheduler(weights: &[&[u32]], retries: u8) -> AttemptScheduler {
        let services: Vec<_> = weights.iter().map(|weights| ServiceCredentials { weights }).collect();
        AttemptScheduler::new(&services, retries).unwrap()
    }

    fn choose(scheduler: &mut AttemptScheduler, service: u8, eligible: u16, entropy: u64) -> AttemptTarget {
        scheduler.select_service(service).unwrap();
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
    fn single_service_retries_in_a_new_round() {
        let mut scheduler = scheduler(&[&[1]], 1);
        assert_eq!(
            choose(&mut scheduler, 0, u16::MAX, 0),
            AttemptTarget {
                service: 0,
                credential: 0
            }
        );
        let failed = dispatch_failure(&mut scheduler, DispatchOutcome::RetryableFailure);
        assert!(matches!(failed.next, NextStep::PickCredential(_)));

        let exhausted = scheduler.select_credential(u16::MAX, 0).unwrap();
        assert_eq!(exhausted.event, SchedulerEvent::ServiceExhausted { service: 0 });
        let advanced = scheduler.complete_service_scan().unwrap();
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
                service: 0,
                credential: 0
            }
        );
    }

    #[test]
    fn rotates_weighted_credentials_before_reselecting_service() {
        let mut scheduler = scheduler(&[&[1, 9], &[1]], 3);
        scheduler.select_service(0).unwrap();
        let selected = scheduler.select_credential(u16::MAX, 5).unwrap();
        assert_eq!(
            selected.next,
            NextStep::Dispatch(AttemptTarget {
                service: 0,
                credential: 1
            })
        );
        dispatch_failure(&mut scheduler, DispatchOutcome::RetryableFailure);

        let selected = scheduler.select_credential(u16::MAX, 5).unwrap();
        assert_eq!(
            selected.next,
            NextStep::Dispatch(AttemptTarget {
                service: 0,
                credential: 0
            })
        );
        dispatch_failure(&mut scheduler, DispatchOutcome::RetryableFailure);

        let exhausted = scheduler.select_credential(u16::MAX, 0).unwrap();
        assert_eq!(exhausted.event, SchedulerEvent::ServiceExhausted { service: 0 });
        assert_eq!(
            exhausted.next,
            NextStep::PickService(PickService {
                round: 0,
                excluded_services: 1,
            })
        );
        assert_eq!(
            choose(&mut scheduler, 1, u16::MAX, 0),
            AttemptTarget {
                service: 1,
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
        let stopped = scheduler.complete_service_scan().unwrap();
        assert_eq!(stopped.event, SchedulerEvent::ScanUnavailable);
        assert_eq!(stopped.next, NextStep::Stop(StopReason::Unavailable));
        assert_eq!(scheduler.round(), 0);
    }

    #[test]
    fn service_wide_predispatch_skip_avoids_scanning_its_credentials() {
        let mut scheduler = scheduler(&[&[1, 1], &[1]], 0);
        choose(&mut scheduler, 0, u16::MAX, 0);
        let skipped = scheduler.skip_predispatch_service().unwrap();
        assert_eq!(skipped.event, SchedulerEvent::PreDispatchServiceSkipped { service: 0 });
        assert_eq!(
            skipped.next,
            NextStep::PickService(PickService {
                round: 0,
                excluded_services: 1,
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
            scheduler.complete_service_scan().unwrap();
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
        let too_many_services = vec![ServiceCredentials { weights: &[] }; MAX_AI_SERVICES + 1];
        assert!(matches!(
            AttemptScheduler::new(&too_many_services, 0),
            Err(SchedulerError::TooManyServices)
        ));
        let too_many_credentials = [1; MAX_AI_CREDENTIALS_PER_SERVICE + 1];
        assert!(matches!(
            AttemptScheduler::new(
                &[ServiceCredentials {
                    weights: &too_many_credentials,
                }],
                0
            ),
            Err(SchedulerError::TooManyCredentials { service: 0 })
        ));
        assert!(matches!(
            AttemptScheduler::new(&[], MAX_AI_ROUTE_RETRIES + 1),
            Err(SchedulerError::TooManyRetries)
        ));
    }
}
