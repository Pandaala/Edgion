//! Metric-free passive-ejection state shared by backend endpoints and AI credentials.

use super::config::OutlierDetectionResolved;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Classification of a recorded upstream failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    Gateway,
    Application,
    LocalOrigin,
}

impl FailureClass {
    #[inline]
    pub fn from_http_status(status: u16) -> Option<Self> {
        if status < 500 {
            return None;
        }
        Some(match status {
            502..=504 => Self::Gateway,
            _ => Self::Application,
        })
    }

    #[inline]
    pub fn metric_label(self) -> &'static str {
        match self {
            Self::Gateway => "gateway",
            Self::Application => "application",
            Self::LocalOrigin => "local_origin",
        }
    }
}

#[derive(Debug, Default)]
struct State {
    app_failures: u32,
    gateway_failures: u32,
    local_origin_failures: u32,
    /// Consecutive-failure ejection window. An in-flight success may clear it.
    open_until: Option<Instant>,
    /// Explicit provider cooldown. An unrelated in-flight success must not
    /// cancel it; only a generation-valid half-open success closes it.
    explicit_cooldown_until: Option<Instant>,
    num_ejections: u32,
    generation: u64,
}

impl State {
    fn effective_open_until(&self) -> Option<Instant> {
        match (self.open_until, self.explicit_cooldown_until) {
            (Some(failure), Some(explicit)) => Some(failure.max(explicit)),
            (Some(until), None) | (None, Some(until)) => Some(until),
            (None, None) => None,
        }
    }
}

/// Result of recording a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureTransition {
    None,
    Opened,
}

/// Opaque proof that a recorded failure reached an ejection threshold in one
/// state generation. Wrappers use it to defer shared percent arbitration until
/// an endpoint is actually eligible to open.
#[derive(Debug)]
pub(crate) struct FailureCandidate {
    generation: u64,
}

/// Admission result for a target using half-open recovery.
pub enum EjectionAdmission {
    Closed,
    Rejected,
    Probe(HalfOpenProbe),
}

/// One exclusive half-open probe. Dropping it without a verdict releases the
/// lease while leaving the target half-open for a later request.
pub struct HalfOpenProbe {
    state: Arc<PassiveEjectionState>,
    generation: u64,
    completed: bool,
}

impl HalfOpenProbe {
    pub fn success(mut self) {
        self.state.finish_probe(self.generation, true, None);
        self.completed = true;
    }

    pub fn failure(mut self, class: FailureClass) {
        self.state.finish_probe(self.generation, false, Some(class));
        self.completed = true;
    }
}

impl Drop for HalfOpenProbe {
    fn drop(&mut self) {
        if !self.completed {
            self.state.release_probe(self.generation);
        }
    }
}

/// A bounded, panic-safe Closed/Open/HalfOpen passive-ejection state machine.
///
/// This type deliberately emits no metrics and knows no resource identity.
/// Wrappers own policy arbitration and observability, which keeps Service
/// metric labels unchanged and prevents AI credential aliases from becoming
/// labels.
pub struct PassiveEjectionState {
    config: Option<OutlierDetectionResolved>,
    inner: Mutex<State>,
    probe_in_flight: AtomicBool,
}

impl PassiveEjectionState {
    pub fn new(config: Option<OutlierDetectionResolved>) -> Arc<Self> {
        Arc::new(Self {
            config,
            inner: Mutex::new(State::default()),
            probe_in_flight: AtomicBool::new(false),
        })
    }

    pub fn config(&self) -> Option<&OutlierDetectionResolved> {
        self.config.as_ref()
    }

    /// Admit a normal request or acquire the sole half-open probe after the
    /// open window expires.
    pub fn admit(self: &Arc<Self>) -> EjectionAdmission {
        let (generation, expired) = {
            let state = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            match state.effective_open_until() {
                None => return EjectionAdmission::Closed,
                Some(until) => (state.generation, Instant::now() >= until),
            }
        };
        if !expired {
            return EjectionAdmission::Rejected;
        }
        if self
            .probe_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return EjectionAdmission::Rejected;
        }

        // A success, config replacement, or new cooldown may have raced the CAS.
        let valid = {
            let state = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            state.generation == generation
                && state
                    .effective_open_until()
                    .is_some_and(|until| Instant::now() >= until)
        };
        if !valid {
            self.probe_in_flight.store(false, Ordering::Release);
            return if self.is_open() {
                EjectionAdmission::Rejected
            } else {
                EjectionAdmission::Closed
            };
        }

        EjectionAdmission::Probe(HalfOpenProbe {
            state: Arc::clone(self),
            generation,
            completed: false,
        })
    }

    /// Record a classified failure. `allow_open` is supplied by the wrapper's
    /// max-ejection-percent arbitration. Counters still advance when opening is
    /// denied, matching the existing Service behavior.
    pub fn record_failure(&self, class: FailureClass, allow_open: bool) -> FailureTransition {
        let Some(candidate) = self.record_failure_candidate(class) else {
            return FailureTransition::None;
        };
        self.finish_failure(candidate, allow_open)
    }

    /// Advance the endpoint-local counter and return a generation-bound
    /// candidate only when an ejection threshold is reached.
    pub(crate) fn record_failure_candidate(&self, class: FailureClass) -> Option<FailureCandidate> {
        let Some(config) = self.config.as_ref() else {
            return None;
        };
        let mut state = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.effective_open_until().is_some() {
            return None;
        }
        match class {
            FailureClass::Application => state.app_failures = state.app_failures.saturating_add(1),
            FailureClass::Gateway => state.gateway_failures = state.gateway_failures.saturating_add(1),
            FailureClass::LocalOrigin => state.local_origin_failures = state.local_origin_failures.saturating_add(1),
        }
        let threshold_reached = state.app_failures >= config.consecutive_errors
            || state.gateway_failures >= config.consecutive_gateway_errors
            || state.local_origin_failures >= config.consecutive_local_origin_failures;
        threshold_reached.then_some(FailureCandidate {
            generation: state.generation,
        })
    }

    /// Open a previously identified failure candidate after the wrapper has
    /// completed shared percent arbitration. Revalidation prevents a stale
    /// candidate from reopening state cleared by a concurrent success.
    pub(crate) fn finish_failure(&self, candidate: FailureCandidate, allow_open: bool) -> FailureTransition {
        if !allow_open {
            return FailureTransition::None;
        }
        let Some(config) = self.config.as_ref() else {
            return FailureTransition::None;
        };
        let mut state = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let threshold_reached = state.app_failures >= config.consecutive_errors
            || state.gateway_failures >= config.consecutive_gateway_errors
            || state.local_origin_failures >= config.consecutive_local_origin_failures;
        if state.generation != candidate.generation || state.effective_open_until().is_some() || !threshold_reached {
            return FailureTransition::None;
        }
        state.num_ejections = state.num_ejections.saturating_add(1);
        let duration = ejection_duration(config, state.num_ejections);
        state.open_until = Some(instant_deadline(duration));
        state.generation = state.generation.wrapping_add(1);
        self.probe_in_flight.store(false, Ordering::Release);
        FailureTransition::Opened
    }

    /// Apply a bounded explicit cooldown, such as a provider Retry-After hint.
    pub fn open_for(&self, duration: Duration) {
        let mut state = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let until = instant_deadline(duration);
        state.explicit_cooldown_until = Some(
            state
                .explicit_cooldown_until
                .map_or(until, |current| current.max(until)),
        );
        state.generation = state.generation.wrapping_add(1);
        self.probe_in_flight.store(false, Ordering::Release);
    }

    /// Clear consecutive-failure state after a normal successful request.
    /// Explicit cooldowns belong to the provider and survive unrelated
    /// in-flight successes until their half-open probe completes.
    pub fn record_success(&self) -> bool {
        let mut state = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let was_open = state.open_until.take().is_some();
        state.app_failures = 0;
        state.gateway_failures = 0;
        state.local_origin_failures = 0;
        // An explicit cooldown owns its half-open generation. A success from a
        // request dispatched before that cooldown must neither invalidate the
        // active probe nor release its exclusive lease.
        if state.explicit_cooldown_until.is_none() {
            state.generation = state.generation.wrapping_add(1);
            self.probe_in_flight.store(false, Ordering::Release);
        }
        was_open
    }

    /// Whether the target is in Open or HalfOpen. This quiet inspection is for
    /// wrapper arbitration and cleanup; it does not acquire a probe.
    pub fn is_open(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .effective_open_until()
            .is_some()
    }

    /// Compatibility read for existing Service selection. It preserves the
    /// historical automatic recovery until Service callsites retain probe guards.
    pub fn legacy_ejection_status(&self) -> (bool, bool) {
        let mut state = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(until) = state.effective_open_until() else {
            return (false, false);
        };
        if Instant::now() < until {
            return (true, false);
        }
        state.open_until = None;
        state.explicit_cooldown_until = None;
        state.app_failures = 0;
        state.gateway_failures = 0;
        state.local_origin_failures = 0;
        state.generation = state.generation.wrapping_add(1);
        self.probe_in_flight.store(false, Ordering::Release);
        (false, true)
    }

    /// Atomically close an expired legacy cooldown while retaining backoff
    /// history. Identity-bound Service runtime calls this at the same expiry
    /// edge that clears its lock-free gate and gauge.
    pub(crate) fn expire_legacy_ejection(&self) -> bool {
        let mut state = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(until) = state.effective_open_until() else {
            return false;
        };
        if Instant::now() < until {
            return false;
        }
        state.open_until = None;
        state.explicit_cooldown_until = None;
        state.app_failures = 0;
        state.gateway_failures = 0;
        state.local_origin_failures = 0;
        state.generation = state.generation.wrapping_add(1);
        self.probe_in_flight.store(false, Ordering::Release);
        true
    }

    /// Remaining duration of the current legacy ejection window.
    ///
    /// Identity-bound Service runtime state copies this cold completion result
    /// into an atomic deadline so request selection never locks this state.
    pub(crate) fn legacy_ejection_remaining(&self) -> Option<Duration> {
        let state = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .effective_open_until()
            .map(|until| until.saturating_duration_since(Instant::now()))
    }

    #[cfg(test)]
    pub(crate) fn num_ejections(&self) -> u32 {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .num_ejections
    }

    /// Move active open deadlines to an already-expired instant without
    /// changing counters, generation, or the half-open probe lease.
    #[cfg(test)]
    pub(crate) fn expire_deadlines_for_test(&self) {
        let mut state = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let expired = Instant::now();
        if state.open_until.is_some() {
            state.open_until = Some(expired);
        }
        if state.explicit_cooldown_until.is_some() {
            state.explicit_cooldown_until = Some(expired);
        }
    }

    fn finish_probe(&self, generation: u64, success: bool, class: Option<FailureClass>) {
        let mut state = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.generation != generation || !self.probe_in_flight.load(Ordering::Acquire) {
            return;
        }
        if success {
            state.open_until = None;
            state.explicit_cooldown_until = None;
            state.app_failures = 0;
            state.gateway_failures = 0;
            state.local_origin_failures = 0;
        } else if let Some(config) = self.config.as_ref() {
            state.num_ejections = state.num_ejections.saturating_add(1);
            state.open_until = Some(instant_deadline(ejection_duration(config, state.num_ejections)));
            if let Some(class) = class {
                match class {
                    FailureClass::Application => state.app_failures = 1,
                    FailureClass::Gateway => state.gateway_failures = 1,
                    FailureClass::LocalOrigin => state.local_origin_failures = 1,
                }
            }
        }
        state.generation = state.generation.wrapping_add(1);
        self.probe_in_flight.store(false, Ordering::Release);
    }

    fn release_probe(&self, generation: u64) {
        let state = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.generation == generation {
            self.probe_in_flight.store(false, Ordering::Release);
        }
    }
}

#[inline]
fn instant_deadline(duration: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(duration).unwrap_or(now)
}

#[inline]
pub fn ejection_duration(config: &OutlierDetectionResolved, num_ejections: u32) -> Duration {
    Duration::from_secs(
        config
            .ejection_seconds
            .saturating_mul(u64::from(num_ejections))
            .min(config.max_ejection_seconds),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    fn config() -> OutlierDetectionResolved {
        OutlierDetectionResolved {
            consecutive_errors: 1,
            consecutive_gateway_errors: 1,
            consecutive_local_origin_failures: 1,
            ejection_seconds: 1,
            max_ejection_seconds: 3,
            max_ejection_percent: 100,
        }
    }

    #[test]
    fn exactly_one_half_open_probe_is_admitted() {
        let state = PassiveEjectionState::new(Some(config()));
        state.open_for(Duration::ZERO);
        let barrier = Arc::new(Barrier::new(8));
        let admitted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let state = Arc::clone(&state);
                let barrier = Arc::clone(&barrier);
                let admitted = Arc::clone(&admitted);
                std::thread::spawn(move || {
                    barrier.wait();
                    if let EjectionAdmission::Probe(probe) = state.admit() {
                        admitted.fetch_add(1, Ordering::Relaxed);
                        std::thread::sleep(Duration::from_millis(20));
                        drop(probe);
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(admitted.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn dropped_probe_releases_without_verdict() {
        let state = PassiveEjectionState::new(Some(config()));
        state.open_for(Duration::ZERO);
        let EjectionAdmission::Probe(probe) = state.admit() else {
            panic!("first probe must be admitted");
        };
        drop(probe);
        assert!(matches!(state.admit(), EjectionAdmission::Probe(_)));
    }

    #[test]
    fn probe_success_closes_and_failure_reopens_with_backoff() {
        let state = PassiveEjectionState::new(Some(config()));
        assert_eq!(
            state.record_failure(FailureClass::Gateway, true),
            FailureTransition::Opened
        );
        assert_eq!(state.num_ejections(), 1);
        state.expire_deadlines_for_test();
        let EjectionAdmission::Probe(probe) = state.admit() else {
            panic!("probe must be admitted after the first window");
        };
        probe.failure(FailureClass::Gateway);
        assert_eq!(state.num_ejections(), 2);
        assert!(matches!(state.admit(), EjectionAdmission::Rejected));

        state.expire_deadlines_for_test();
        let EjectionAdmission::Probe(probe) = state.admit() else {
            panic!("probe must be admitted after backoff");
        };
        probe.success();
        assert!(matches!(state.admit(), EjectionAdmission::Closed));
    }

    #[test]
    fn disabled_thresholds_still_support_explicit_cooldown() {
        let state = PassiveEjectionState::new(None);
        assert_eq!(
            state.record_failure(FailureClass::Gateway, true),
            FailureTransition::None
        );
        assert!(matches!(state.admit(), EjectionAdmission::Closed));
        state.open_for(Duration::from_secs(1));
        assert!(matches!(state.admit(), EjectionAdmission::Rejected));
    }

    #[test]
    fn normal_success_preserves_an_explicit_cooldown() {
        let state = PassiveEjectionState::new(Some(config()));
        state.open_for(Duration::from_secs(60));

        assert!(!state.record_success(), "no failure ejection was cleared");
        assert!(matches!(state.admit(), EjectionAdmission::Rejected));
    }

    #[test]
    fn normal_success_does_not_invalidate_an_explicit_half_open_probe() {
        let state = PassiveEjectionState::new(Some(config()));
        state.open_for(Duration::ZERO);
        let EjectionAdmission::Probe(probe) = state.admit() else {
            panic!("expired explicit cooldown must admit one probe");
        };

        assert!(!state.record_success(), "no failure ejection was cleared");
        assert!(matches!(state.admit(), EjectionAdmission::Rejected));

        probe.success();
        assert!(matches!(state.admit(), EjectionAdmission::Closed));
    }

    #[test]
    fn normal_success_still_clears_a_failure_ejection() {
        let state = PassiveEjectionState::new(Some(config()));
        assert_eq!(
            state.record_failure(FailureClass::Gateway, true),
            FailureTransition::Opened
        );

        assert!(state.record_success());
        assert!(matches!(state.admit(), EjectionAdmission::Closed));
    }

    #[test]
    fn stale_failure_candidate_cannot_reopen_after_success() {
        let state = PassiveEjectionState::new(Some(config()));
        let candidate = state
            .record_failure_candidate(FailureClass::Gateway)
            .expect("threshold-one failure must become a candidate");

        assert!(!state.record_success());
        assert_eq!(state.finish_failure(candidate, true), FailureTransition::None);
        assert!(matches!(state.admit(), EjectionAdmission::Closed));
    }
}
