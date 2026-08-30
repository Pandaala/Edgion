//! Bounded, failure-open observation of the canonical proxied request body.
//!
//! This is an internal extension seam, not a second body cache. Observers see
//! borrowed post-mutation bytes synchronously and can neither alter forwarding
//! nor retain the transport `Bytes` object.

#![allow(dead_code, reason = "WP5 installs the seam; WP6 adds its first production observer")]

use crate::ctx::CurrentPluginEntryIdentity;
use crate::plugins::runtime::PluginLog;
use edgion_common::startup::catch_redacted_panic;
use pingora_proxy::RequestBodyEvent;

pub(crate) const MAX_REQUEST_BODY_OBSERVERS: usize = 4;
pub(crate) const MAX_OBSERVER_BYTES: usize = 64 * 1024;
pub(crate) const MAX_OBSERVER_EVENTS: usize = 4_096;
pub(crate) const MAX_OBSERVER_WORK: usize = 262_144;
pub(crate) const MAX_OBSERVER_RETAINED_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ObserverLimits {
    bytes: usize,
    events: usize,
    work: usize,
    retained_bytes: usize,
}

impl ObserverLimits {
    pub(crate) fn new(bytes: usize, events: usize, work: usize, retained_bytes: usize) -> Option<Self> {
        if bytes == 0 || events == 0 || work == 0 || retained_bytes == 0 {
            return None;
        }
        Some(Self {
            bytes: bytes.min(MAX_OBSERVER_BYTES),
            events: events.min(MAX_OBSERVER_EVENTS),
            work: work.min(MAX_OBSERVER_WORK),
            retained_bytes: retained_bytes.min(MAX_OBSERVER_RETAINED_BYTES),
        })
    }
}

impl Default for ObserverLimits {
    fn default() -> Self {
        Self {
            bytes: MAX_OBSERVER_BYTES,
            events: MAX_OBSERVER_EVENTS,
            work: MAX_OBSERVER_WORK,
            retained_bytes: MAX_OBSERVER_RETAINED_BYTES,
        }
    }
}

/// Request-wide host budget. Retry resets semantic state, never these counters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ObserverBudget {
    remaining_bytes: usize,
    remaining_events: usize,
    remaining_work: usize,
    exhausted: bool,
}

impl ObserverBudget {
    fn new(limits: ObserverLimits) -> Self {
        Self {
            remaining_bytes: limits.bytes,
            remaining_events: limits.events,
            remaining_work: limits.work,
            exhausted: false,
        }
    }

    /// Charge semantic parser work before performing it.
    #[must_use]
    pub(crate) fn charge_work(&mut self, units: usize) -> bool {
        if self.exhausted || units > self.remaining_work {
            self.latch_exhausted();
            return false;
        }
        self.remaining_work -= units;
        true
    }

    fn charge_callback(&mut self, bytes: usize) -> bool {
        if self.exhausted || bytes > self.remaining_bytes || self.remaining_events == 0 || self.remaining_work == 0 {
            self.latch_exhausted();
            return false;
        }
        self.remaining_bytes -= bytes;
        self.remaining_events -= 1;
        self.remaining_work -= 1;
        true
    }

    fn latch_exhausted(&mut self) {
        self.exhausted = true;
        self.remaining_bytes = 0;
        self.remaining_events = 0;
        self.remaining_work = 0;
    }
}

pub(crate) mod sealed {
    /// Implemented only by trusted observers compiled into the gateway.
    pub trait Sealed {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ObserverControl {
    Continue,
    SemanticReady,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ObserverTerminal {
    Complete,
    Incomplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ObserverError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ObserverInitError;

/// Trusted synchronous request-body observer.
pub(crate) trait RequestBodyObserver: sealed::Sealed + Send + Sync {
    /// Fixed scratch retained by the implementation, excluding the trait box.
    fn retained_bytes(&self) -> usize;

    fn observe(
        &mut self,
        bytes: &[u8],
        event: RequestBodyEvent,
        budget: &mut ObserverBudget,
    ) -> Result<ObserverControl, ObserverError>;

    fn reset_for_attempt(&mut self, _budget: &mut ObserverBudget) -> Result<(), ObserverError> {
        Ok(())
    }

    fn finalize(&mut self, _terminal: ObserverTerminal, _budget: &mut ObserverBudget) -> Result<(), ObserverError> {
        Ok(())
    }
}

pub(crate) type RequestBodyObserverFactory =
    Box<dyn FnOnce() -> Result<Box<dyn RequestBodyObserver>, ObserverInitError> + Send>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ObserverOutcome {
    Complete,
    Incomplete,
    Exhausted,
    Failed,
    Panicked,
}

impl ObserverOutcome {
    fn label(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Incomplete => "incomplete",
            Self::Exhausted => "exhausted",
            Self::Failed => "failed",
            Self::Panicked => "panicked",
        }
    }
}

struct ObserverSlot {
    observer: Option<Box<dyn RequestBodyObserver>>,
    log: Option<PluginLog>,
    budget: ObserverBudget,
    retained_limit: usize,
    last_attempt: Option<u32>,
    observed_event: bool,
    semantic_ready: bool,
    outcome: Option<ObserverOutcome>,
}

impl ObserverSlot {
    fn check_retained(&mut self) -> bool {
        let Some(observer) = self.observer.as_ref() else {
            return true;
        };
        match catch_redacted_panic(|| observer.retained_bytes()) {
            Ok(bytes) if bytes <= self.retained_limit => true,
            Ok(_) => {
                self.finish_diagnostic(ObserverOutcome::Exhausted);
                false
            }
            Err(_) => {
                self.finish_diagnostic(ObserverOutcome::Panicked);
                false
            }
        }
    }

    fn protected_drop_observer(&mut self) -> bool {
        let Some(observer) = self.observer.take() else {
            return true;
        };
        catch_redacted_panic(|| drop(observer)).is_ok()
    }

    fn finish_diagnostic(&mut self, outcome: ObserverOutcome) {
        if self.outcome.is_some() {
            return;
        }
        if outcome == ObserverOutcome::Exhausted {
            self.budget.latch_exhausted();
        }
        self.outcome = Some(if self.protected_drop_observer() {
            outcome
        } else {
            ObserverOutcome::Panicked
        });
    }

    fn invoke_reset(&mut self) -> bool {
        if !self.budget.charge_callback(0) {
            self.finish_diagnostic(ObserverOutcome::Exhausted);
            return false;
        }
        let result = {
            let observer = self.observer.as_mut().expect("active slot has observer");
            catch_redacted_panic(|| observer.reset_for_attempt(&mut self.budget))
        };
        match result {
            Err(_) => self.finish_diagnostic(ObserverOutcome::Panicked),
            Ok(Err(_)) => self.finish_diagnostic(ObserverOutcome::Failed),
            Ok(Ok(())) if self.budget.exhausted => self.finish_diagnostic(ObserverOutcome::Exhausted),
            Ok(Ok(())) => {
                self.semantic_ready = false;
                if self.check_retained() {
                    return true;
                }
            }
        }
        false
    }

    fn finalize(&mut self, terminal: ObserverTerminal) {
        if self.outcome.is_some() {
            return;
        }
        if !self.budget.charge_callback(0) {
            self.finish_diagnostic(ObserverOutcome::Exhausted);
            return;
        }
        let result = {
            let observer = self.observer.as_mut().expect("active slot has observer");
            catch_redacted_panic(|| observer.finalize(terminal, &mut self.budget))
        };
        match result {
            Err(_) => self.finish_diagnostic(ObserverOutcome::Panicked),
            Ok(Err(_)) => self.finish_diagnostic(ObserverOutcome::Failed),
            Ok(Ok(())) if self.budget.exhausted => self.finish_diagnostic(ObserverOutcome::Exhausted),
            Ok(Ok(())) if !self.check_retained() => {}
            Ok(Ok(())) => {
                let desired = match terminal {
                    ObserverTerminal::Complete => ObserverOutcome::Complete,
                    ObserverTerminal::Incomplete => ObserverOutcome::Incomplete,
                };
                if self.protected_drop_observer() {
                    self.outcome = Some(desired);
                } else {
                    self.outcome = Some(ObserverOutcome::Panicked);
                }
            }
        }
    }

    fn dispatch(&mut self, bytes: &[u8], event: RequestBodyEvent, attempt: u32) {
        if self.outcome.is_some() {
            return;
        }

        if self.observed_event && self.last_attempt != Some(attempt) && !self.invoke_reset() {
            return;
        }
        self.last_attempt = Some(attempt);

        // Semantic readiness may skip later data-only events, but terminal
        // event bytes are always delivered before finalization. A transport
        // can attach the last bytes to Complete/Abandoned.
        if !self.semantic_ready || event.is_terminal() {
            if !self.budget.charge_callback(bytes.len()) {
                self.finish_diagnostic(ObserverOutcome::Exhausted);
                return;
            }
            self.observed_event = true;
            let result = {
                let observer = self.observer.as_mut().expect("active slot has observer");
                catch_redacted_panic(|| observer.observe(bytes, event, &mut self.budget))
            };
            match result {
                Err(_) => {
                    self.finish_diagnostic(ObserverOutcome::Panicked);
                    return;
                }
                Ok(Err(_)) => {
                    self.finish_diagnostic(ObserverOutcome::Failed);
                    return;
                }
                Ok(Ok(_)) if self.budget.exhausted => {
                    self.finish_diagnostic(ObserverOutcome::Exhausted);
                    return;
                }
                Ok(Ok(ObserverControl::SemanticReady)) => self.semantic_ready = true,
                Ok(Ok(ObserverControl::Continue)) => {}
            }
            if !self.check_retained() {
                return;
            }
        }

        match event {
            RequestBodyEvent::Complete => self.finalize(ObserverTerminal::Complete),
            RequestBodyEvent::Abandoned => self.finalize(ObserverTerminal::Incomplete),
            RequestBodyEvent::Data => {}
        }
    }
}

impl Drop for ObserverSlot {
    fn drop(&mut self) {
        let _ = self.protected_drop_observer();
    }
}

/// Fixed-capacity registry allocated only after an observer registration is attempted.
pub(crate) struct RequestBodyObservers {
    slots: [Option<ObserverSlot>; MAX_REQUEST_BODY_OBSERVERS],
    len: usize,
}

impl Default for RequestBodyObservers {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| None),
            len: 0,
        }
    }
}

impl RequestBodyObservers {
    pub(crate) fn is_full(&self) -> bool {
        self.len == MAX_REQUEST_BODY_OBSERVERS
    }

    pub(crate) fn try_install(
        &mut self,
        identity: CurrentPluginEntryIdentity,
        factory: RequestBodyObserverFactory,
        limits: ObserverLimits,
    ) -> bool {
        if self.is_full() {
            let _ = catch_redacted_panic(|| drop(factory));
            return false;
        }

        let observer = match catch_redacted_panic(factory) {
            Ok(Ok(observer)) => observer,
            Ok(Err(_)) | Err(_) => return false,
        };
        let retained = match catch_redacted_panic(|| observer.retained_bytes()) {
            Ok(retained) => retained,
            Err(_) => {
                let _ = catch_redacted_panic(|| drop(observer));
                return false;
            }
        };
        if retained > limits.retained_bytes {
            let _ = catch_redacted_panic(|| drop(observer));
            return false;
        }

        self.slots[self.len] = Some(ObserverSlot {
            observer: Some(observer),
            log: Some(PluginLog::with_alias(identity.name(), identity.alias())),
            budget: ObserverBudget::new(limits),
            retained_limit: limits.retained_bytes,
            last_attempt: None,
            observed_event: false,
            semantic_ready: false,
            outcome: None,
        });
        self.len += 1;
        true
    }

    /// Observe a transport event without copying or mutating its bytes.
    pub(crate) fn observe(&mut self, bytes: &[u8], event: RequestBodyEvent, attempt: u32) {
        for slot in self.slots[..self.len].iter_mut().flatten() {
            slot.dispatch(bytes, event, attempt);
        }
    }

    /// Idempotently finish active slots and transfer their bounded logs.
    pub(crate) fn finalize_incomplete_and_take_logs(&mut self) -> Vec<PluginLog> {
        let mut logs = Vec::with_capacity(self.len);
        for slot in self.slots[..self.len].iter_mut().flatten() {
            slot.finalize(ObserverTerminal::Incomplete);
            if let (Some(mut log), Some(outcome)) = (slot.log.take(), slot.outcome) {
                log.push(outcome.label());
                logs.push(log);
            }
        }
        logs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct State {
        bodies: Vec<Vec<u8>>,
        resets: usize,
        finalizes: Vec<ObserverTerminal>,
        drops: usize,
    }

    struct TestObserver {
        state: Arc<Mutex<State>>,
        retained: usize,
        control: ObserverControl,
        fail: bool,
        panic: bool,
        semantic_charge: usize,
    }

    struct DropPanicObserver;

    impl sealed::Sealed for DropPanicObserver {}

    impl RequestBodyObserver for DropPanicObserver {
        fn retained_bytes(&self) -> usize {
            0
        }

        fn observe(
            &mut self,
            _bytes: &[u8],
            _event: RequestBodyEvent,
            _budget: &mut ObserverBudget,
        ) -> Result<ObserverControl, ObserverError> {
            Ok(ObserverControl::Continue)
        }
    }

    impl Drop for DropPanicObserver {
        fn drop(&mut self) {
            panic!("static destructor panic");
        }
    }

    struct QueryPanicObserver {
        queries: std::sync::atomic::AtomicUsize,
        panic_after: usize,
    }

    impl sealed::Sealed for QueryPanicObserver {}

    impl RequestBodyObserver for QueryPanicObserver {
        fn retained_bytes(&self) -> usize {
            let query = self.queries.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            assert!(query < self.panic_after, "static retained query panic");
            0
        }

        fn observe(
            &mut self,
            _bytes: &[u8],
            _event: RequestBodyEvent,
            _budget: &mut ObserverBudget,
        ) -> Result<ObserverControl, ObserverError> {
            Ok(ObserverControl::Continue)
        }
    }

    struct FailDropPanicObserver;

    impl sealed::Sealed for FailDropPanicObserver {}

    impl RequestBodyObserver for FailDropPanicObserver {
        fn retained_bytes(&self) -> usize {
            0
        }

        fn observe(
            &mut self,
            _bytes: &[u8],
            _event: RequestBodyEvent,
            _budget: &mut ObserverBudget,
        ) -> Result<ObserverControl, ObserverError> {
            Err(ObserverError)
        }
    }

    impl Drop for FailDropPanicObserver {
        fn drop(&mut self) {
            panic!("static failing destructor panic");
        }
    }

    struct NoAllocObserver {
        seen: usize,
    }

    #[derive(Clone, Copy)]
    enum LifecycleFault {
        None,
        Error,
        Panic,
    }

    struct LifecycleFaultObserver {
        reset: LifecycleFault,
        finalize: LifecycleFault,
    }

    impl sealed::Sealed for LifecycleFaultObserver {}

    impl RequestBodyObserver for LifecycleFaultObserver {
        fn retained_bytes(&self) -> usize {
            1
        }

        fn observe(
            &mut self,
            _bytes: &[u8],
            _event: RequestBodyEvent,
            _budget: &mut ObserverBudget,
        ) -> Result<ObserverControl, ObserverError> {
            Ok(ObserverControl::Continue)
        }

        fn reset_for_attempt(&mut self, _budget: &mut ObserverBudget) -> Result<(), ObserverError> {
            match self.reset {
                LifecycleFault::None => Ok(()),
                LifecycleFault::Error => Err(ObserverError),
                LifecycleFault::Panic => panic!("static reset panic"),
            }
        }

        fn finalize(&mut self, _terminal: ObserverTerminal, _budget: &mut ObserverBudget) -> Result<(), ObserverError> {
            match self.finalize {
                LifecycleFault::None => Ok(()),
                LifecycleFault::Error => Err(ObserverError),
                LifecycleFault::Panic => panic!("static finalize panic"),
            }
        }
    }

    struct GrowingRetainedObserver {
        queried: std::sync::atomic::AtomicBool,
    }

    impl sealed::Sealed for GrowingRetainedObserver {}

    impl RequestBodyObserver for GrowingRetainedObserver {
        fn retained_bytes(&self) -> usize {
            if self.queried.swap(true, std::sync::atomic::Ordering::Relaxed) {
                MAX_OBSERVER_RETAINED_BYTES + 1
            } else {
                1
            }
        }

        fn observe(
            &mut self,
            _bytes: &[u8],
            _event: RequestBodyEvent,
            _budget: &mut ObserverBudget,
        ) -> Result<ObserverControl, ObserverError> {
            Ok(ObserverControl::Continue)
        }
    }

    impl sealed::Sealed for NoAllocObserver {}

    impl RequestBodyObserver for NoAllocObserver {
        fn retained_bytes(&self) -> usize {
            std::mem::size_of::<Self>()
        }

        fn observe(
            &mut self,
            bytes: &[u8],
            _event: RequestBodyEvent,
            budget: &mut ObserverBudget,
        ) -> Result<ObserverControl, ObserverError> {
            if !budget.charge_work(bytes.len()) {
                return Ok(ObserverControl::Continue);
            }
            self.seen = self.seen.saturating_add(bytes.len());
            Ok(ObserverControl::Continue)
        }
    }

    impl sealed::Sealed for TestObserver {}

    impl RequestBodyObserver for TestObserver {
        fn retained_bytes(&self) -> usize {
            self.retained
        }

        fn observe(
            &mut self,
            bytes: &[u8],
            _event: RequestBodyEvent,
            budget: &mut ObserverBudget,
        ) -> Result<ObserverControl, ObserverError> {
            assert!(!self.panic, "static observer panic");
            if self.fail {
                return Err(ObserverError);
            }
            if self.semantic_charge != 0 && !budget.charge_work(self.semantic_charge) {
                return Ok(ObserverControl::Continue);
            }
            self.state.lock().unwrap().bodies.push(bytes.to_vec());
            Ok(self.control)
        }

        fn reset_for_attempt(&mut self, _budget: &mut ObserverBudget) -> Result<(), ObserverError> {
            self.state.lock().unwrap().resets += 1;
            Ok(())
        }

        fn finalize(&mut self, terminal: ObserverTerminal, _budget: &mut ObserverBudget) -> Result<(), ObserverError> {
            self.state.lock().unwrap().finalizes.push(terminal);
            Ok(())
        }
    }

    impl Drop for TestObserver {
        fn drop(&mut self) {
            self.state.lock().unwrap().drops += 1;
        }
    }

    fn identity(name: &str) -> CurrentPluginEntryIdentity {
        CurrentPluginEntryIdentity::new(name, Some("entry-1")).unwrap()
    }

    fn factory(state: Arc<Mutex<State>>) -> RequestBodyObserverFactory {
        Box::new(move || {
            Ok(Box::new(TestObserver {
                state,
                retained: 8,
                control: ObserverControl::Continue,
                fail: false,
                panic: false,
                semantic_charge: 0,
            }))
        })
    }

    fn outcome(registry: &mut RequestBodyObservers) -> String {
        let logs = registry.finalize_incomplete_and_take_logs();
        serde_json::to_string(&logs).unwrap()
    }

    #[test]
    fn limits_reject_zero_and_clamp_to_host_maxima() {
        assert!(ObserverLimits::new(0, 1, 1, 1).is_none());
        let limits = ObserverLimits::new(usize::MAX, usize::MAX, usize::MAX, usize::MAX).unwrap();
        assert_eq!(limits, ObserverLimits::default());
    }

    #[test]
    fn work_exhaustion_is_monotonic_and_latches_the_whole_budget() {
        let limits = ObserverLimits::new(8, 8, 3, 8).unwrap();
        let mut budget = ObserverBudget::new(limits);

        assert!(!budget.charge_work(4));
        assert!(budget.exhausted);
        assert_eq!(budget.remaining_bytes, 0);
        assert_eq!(budget.remaining_events, 0);
        assert_eq!(budget.remaining_work, 0);
        assert!(!budget.charge_work(1));
        assert!(!budget.charge_work(0));
        assert!(!budget.charge_callback(0));
    }

    #[test]
    fn zero_registry_branch_and_registered_dispatch_do_not_allocate() {
        let mut absent: Option<Box<RequestBodyObservers>> = None;
        let (_, absent_allocations) = crate::test_allocation_counter::count_allocations(|| {
            if let Some(observers) = absent.as_mut() {
                observers.observe(b"ordinary", RequestBodyEvent::Data, 1);
            }
        });
        assert_eq!(absent_allocations, 0);
        assert!(absent.is_none());

        let mut registry = RequestBodyObservers::default();
        assert!(registry.try_install(
            identity("no_alloc"),
            Box::new(|| Ok(Box::new(NoAllocObserver { seen: 0 }))),
            ObserverLimits::default(),
        ));
        let (_, dispatch_allocations) = crate::test_allocation_counter::count_allocations(|| {
            registry.observe(b"registered", RequestBodyEvent::Data, 1);
        });
        assert_eq!(dispatch_allocations, 0);
    }

    #[test]
    fn complete_observes_terminal_bytes_and_finalizes_once() {
        let state = Arc::new(Mutex::new(State::default()));
        let mut registry = RequestBodyObservers::default();
        assert!(registry.try_install(identity("test"), factory(state.clone()), ObserverLimits::default()));
        registry.observe(b"abc", RequestBodyEvent::Complete, 1);
        registry.observe(b"replay", RequestBodyEvent::Complete, 2);
        let json = outcome(&mut registry);
        assert!(json.contains("complete"));
        let state = state.lock().unwrap();
        assert_eq!(state.bodies, [b"abc".to_vec()]);
        assert_eq!(state.finalizes, [ObserverTerminal::Complete]);
        assert_eq!(state.drops, 1);
    }

    #[test]
    fn partial_retry_resets_semantics_but_not_total_budget() {
        let state = Arc::new(Mutex::new(State::default()));
        let mut registry = RequestBodyObservers::default();
        let limits = ObserverLimits::new(6, 10, 20, 20).unwrap();
        assert!(registry.try_install(identity("test"), factory(state.clone()), limits));
        registry.observe(b"abc", RequestBodyEvent::Data, 1);
        registry.observe(b"abc", RequestBodyEvent::Data, 2);
        registry.observe(b"x", RequestBodyEvent::Complete, 2);
        let json = outcome(&mut registry);
        assert!(json.contains("exhausted"));
        let state = state.lock().unwrap();
        assert_eq!(state.resets, 1);
        assert_eq!(state.bodies, [b"abc".to_vec(), b"abc".to_vec()]);
    }

    #[test]
    fn event_charge_is_atomic_and_withholds_over_limit_chunk() {
        let state = Arc::new(Mutex::new(State::default()));
        let mut registry = RequestBodyObservers::default();
        let limits = ObserverLimits::new(3, 5, 5, 20).unwrap();
        assert!(registry.try_install(identity("test"), factory(state.clone()), limits));
        registry.observe(b"four", RequestBodyEvent::Data, 1);
        assert!(outcome(&mut registry).contains("exhausted"));
        assert!(state.lock().unwrap().bodies.is_empty());
    }

    #[test]
    fn exact_byte_event_and_work_boundaries_are_inclusive() {
        for (limits, first, second) in [
            (
                ObserverLimits::new(1, 3, 3, 8).unwrap(),
                b"x".as_slice(),
                b"y".as_slice(),
            ),
            (
                ObserverLimits::new(8, 1, 8, 8).unwrap(),
                b"x".as_slice(),
                b"".as_slice(),
            ),
            (
                ObserverLimits::new(8, 8, 1, 8).unwrap(),
                b"x".as_slice(),
                b"".as_slice(),
            ),
        ] {
            let state = Arc::new(Mutex::new(State::default()));
            let mut registry = RequestBodyObservers::default();
            assert!(registry.try_install(identity("boundary"), factory(state.clone()), limits));
            registry.observe(first, RequestBodyEvent::Data, 1);
            assert_eq!(state.lock().unwrap().bodies.len(), 1);
            registry.observe(second, RequestBodyEvent::Data, 1);
            assert!(outcome(&mut registry).contains("exhausted"));
        }
    }

    #[test]
    fn semantic_work_exhaustion_is_failure_open_and_static() {
        let state = Arc::new(Mutex::new(State::default()));
        let mut registry = RequestBodyObservers::default();
        let observer = TestObserver {
            state,
            retained: 1,
            control: ObserverControl::Continue,
            fail: false,
            panic: false,
            semantic_charge: 2,
        };
        let limits = ObserverLimits::new(8, 8, 2, 8).unwrap();
        assert!(registry.try_install(identity("test"), Box::new(|| Ok(Box::new(observer))), limits));
        registry.observe(b"x", RequestBodyEvent::Data, 1);
        assert!(outcome(&mut registry).contains("exhausted"));
    }

    #[test]
    fn observer_failure_and_panic_are_isolated() {
        for (fail, panic, expected) in [(true, false, "failed"), (false, true, "panicked")] {
            let state = Arc::new(Mutex::new(State::default()));
            let observer = TestObserver {
                state,
                retained: 1,
                control: ObserverControl::Continue,
                fail,
                panic,
                semantic_charge: 0,
            };
            let mut registry = RequestBodyObservers::default();
            assert!(registry.try_install(
                identity("test"),
                Box::new(|| Ok(Box::new(observer))),
                ObserverLimits::default(),
            ));
            registry.observe(b"secret", RequestBodyEvent::Data, 1);
            assert!(outcome(&mut registry).contains(expected));
        }
    }

    #[test]
    fn semantic_ready_waits_for_transport_terminal() {
        let state = Arc::new(Mutex::new(State::default()));
        let observer = TestObserver {
            state: state.clone(),
            retained: 1,
            control: ObserverControl::SemanticReady,
            fail: false,
            panic: false,
            semantic_charge: 0,
        };
        let mut registry = RequestBodyObservers::default();
        assert!(registry.try_install(
            identity("test"),
            Box::new(|| Ok(Box::new(observer))),
            ObserverLimits::default(),
        ));
        registry.observe(b"method", RequestBodyEvent::Data, 1);
        registry.observe(b"ignored", RequestBodyEvent::Complete, 1);
        assert!(outcome(&mut registry).contains("complete"));
        assert_eq!(state.lock().unwrap().bodies, [b"method".to_vec(), b"ignored".to_vec()]);
    }

    #[test]
    fn abandoned_and_logging_sweep_are_incomplete_and_idempotent() {
        for terminal in [Some(RequestBodyEvent::Abandoned), None] {
            let state = Arc::new(Mutex::new(State::default()));
            let mut registry = RequestBodyObservers::default();
            assert!(registry.try_install(identity("test"), factory(state), ObserverLimits::default()));
            if let Some(event) = terminal {
                registry.observe(&[], event, 1);
            }
            assert!(outcome(&mut registry).contains("incomplete"));
            assert!(registry.finalize_incomplete_and_take_logs().is_empty());
        }
    }

    #[test]
    fn capacity_rejects_fifth_without_disturbing_existing_slots() {
        let mut registry = RequestBodyObservers::default();
        for _ in 0..MAX_REQUEST_BODY_OBSERVERS {
            assert!(registry.try_install(
                identity("test"),
                factory(Arc::new(Mutex::new(State::default()))),
                ObserverLimits::default(),
            ));
        }
        assert!(!registry.try_install(
            identity("fifth"),
            factory(Arc::new(Mutex::new(State::default()))),
            ObserverLimits::default(),
        ));
        registry.observe(&[], RequestBodyEvent::Complete, 1);
        assert_eq!(
            registry.finalize_incomplete_and_take_logs().len(),
            MAX_REQUEST_BODY_OBSERVERS
        );
    }

    #[test]
    fn construction_failure_panic_and_excess_scratch_reject_registration() {
        let mut registry = RequestBodyObservers::default();
        assert!(!registry.try_install(
            identity("error"),
            Box::new(|| Err(ObserverInitError)),
            ObserverLimits::default(),
        ));
        assert!(!registry.try_install(
            identity("panic"),
            Box::new(|| panic!("static factory panic")),
            ObserverLimits::default(),
        ));
        assert!(!registry.try_install(
            identity("query_panic"),
            Box::new(|| {
                Ok(Box::new(QueryPanicObserver {
                    queries: std::sync::atomic::AtomicUsize::new(0),
                    panic_after: 0,
                }))
            }),
            ObserverLimits::default(),
        ));

        let observer = TestObserver {
            state: Arc::new(Mutex::new(State::default())),
            retained: MAX_OBSERVER_RETAINED_BYTES + 1,
            control: ObserverControl::Continue,
            fail: false,
            panic: false,
            semantic_charge: 0,
        };
        assert!(!registry.try_install(
            identity("large"),
            Box::new(|| Ok(Box::new(observer))),
            ObserverLimits::default(),
        ));
        assert!(!registry.is_full());
        assert!(registry.finalize_incomplete_and_take_logs().is_empty());
    }

    #[test]
    fn one_bad_slot_does_not_stop_later_slots() {
        let failed = TestObserver {
            state: Arc::new(Mutex::new(State::default())),
            retained: 1,
            control: ObserverControl::Continue,
            fail: true,
            panic: false,
            semantic_charge: 0,
        };
        let healthy_state = Arc::new(Mutex::new(State::default()));
        let mut registry = RequestBodyObservers::default();
        assert!(registry.try_install(
            identity("failed"),
            Box::new(|| Ok(Box::new(failed))),
            ObserverLimits::default(),
        ));
        assert!(registry.try_install(
            identity("healthy"),
            factory(healthy_state.clone()),
            ObserverLimits::default(),
        ));
        registry.observe(b"unchanged", RequestBodyEvent::Complete, 1);
        let json = serde_json::to_string(&registry.finalize_incomplete_and_take_logs()).unwrap();
        assert!(json.contains("failed"));
        assert!(json.contains("complete"));
        assert_eq!(healthy_state.lock().unwrap().bodies, [b"unchanged".to_vec()]);
    }

    #[test]
    fn destructor_panic_is_isolated_and_reported_after_successful_finalize() {
        let mut registry = RequestBodyObservers::default();
        assert!(registry.try_install(
            identity("drop_panic"),
            Box::new(|| Ok(Box::new(DropPanicObserver))),
            ObserverLimits::default(),
        ));
        registry.observe(&[], RequestBodyEvent::Complete, 1);
        assert!(outcome(&mut registry).contains("panicked"));
    }

    #[test]
    fn active_registry_drop_isolates_observer_destructor_panic() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut registry = RequestBodyObservers::default();
            assert!(registry.try_install(
                identity("drop_panic"),
                Box::new(|| Ok(Box::new(DropPanicObserver))),
                ObserverLimits::default(),
            ));
            drop(registry);
        }));

        assert!(
            result.is_ok(),
            "observer destructor panic must not escape registry drop"
        );
    }

    #[test]
    fn post_callback_query_and_failed_observer_drop_panics_are_reported() {
        for factory in [
            Box::new(|| {
                Ok(Box::new(QueryPanicObserver {
                    queries: std::sync::atomic::AtomicUsize::new(0),
                    panic_after: 1,
                }) as Box<dyn RequestBodyObserver>)
            }) as RequestBodyObserverFactory,
            Box::new(|| Ok(Box::new(FailDropPanicObserver) as Box<dyn RequestBodyObserver>)),
        ] {
            let mut registry = RequestBodyObservers::default();
            assert!(registry.try_install(identity("panic"), factory, ObserverLimits::default()));
            registry.observe(b"x", RequestBodyEvent::Data, 1);
            assert!(outcome(&mut registry).contains("panicked"));
        }
    }

    #[test]
    fn reset_and_finalize_errors_and_panics_are_isolated() {
        for (reset, finalize, expected) in [
            (LifecycleFault::Error, LifecycleFault::None, "failed"),
            (LifecycleFault::Panic, LifecycleFault::None, "panicked"),
            (LifecycleFault::None, LifecycleFault::Error, "failed"),
            (LifecycleFault::None, LifecycleFault::Panic, "panicked"),
        ] {
            let mut registry = RequestBodyObservers::default();
            assert!(registry.try_install(
                identity("lifecycle"),
                Box::new(move || Ok(Box::new(LifecycleFaultObserver { reset, finalize }))),
                ObserverLimits::default(),
            ));
            registry.observe(b"prefix", RequestBodyEvent::Data, 1);
            if !matches!(reset, LifecycleFault::None) {
                registry.observe(b"replay", RequestBodyEvent::Data, 2);
            } else {
                registry.observe(&[], RequestBodyEvent::Complete, 1);
            }
            assert!(outcome(&mut registry).contains(expected));
        }
    }

    #[test]
    fn post_callback_retained_growth_is_exhausted() {
        let mut registry = RequestBodyObservers::default();
        assert!(registry.try_install(
            identity("growth"),
            Box::new(|| {
                Ok(Box::new(GrowingRetainedObserver {
                    queried: std::sync::atomic::AtomicBool::new(false),
                }))
            }),
            ObserverLimits::default(),
        ));
        registry.observe(b"x", RequestBodyEvent::Data, 1);
        assert!(outcome(&mut registry).contains("exhausted"));
    }
}
