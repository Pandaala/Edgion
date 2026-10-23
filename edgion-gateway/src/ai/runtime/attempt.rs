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
    /// Copy exact-length weight rows into request-owned fixed storage.
    pub fn new<B, C>(backends: B, route_retries: u8) -> Result<Self, SchedulerError>
    where
        B: ExactSizeIterator<Item = C>,
        C: ExactSizeIterator<Item = u32>,
    {
        let backend_count = backends.len();
        if backend_count > MAX_AI_BACKENDS {
            return Err(SchedulerError::TooManyBackends);
        }
        if route_retries > MAX_AI_ROUTE_RETRIES {
            return Err(SchedulerError::TooManyRetries);
        }

        let mut compiled = [EMPTY_BACKEND; MAX_AI_BACKENDS];
        for (backend_index, weights) in backends.enumerate() {
            let credential_count = weights.len();
            if credential_count > MAX_AI_CREDENTIALS_PER_BACKEND {
                return Err(SchedulerError::TooManyCredentials {
                    backend: backend_index as u8,
                });
            }
            compiled[backend_index].credential_count = credential_count as u8;
            for (slot, weight) in compiled[backend_index].weights[..credential_count]
                .iter_mut()
                .zip(weights)
            {
                *slot = weight;
            }
        }

        Ok(Self {
            backends: compiled,
            backend_count: backend_count as u8,
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
#[path = "../../../tests/unit/ai/runtime/attempt_test.rs"]
mod tests;
