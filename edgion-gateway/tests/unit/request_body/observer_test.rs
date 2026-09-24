use super::*;
use std::sync::Arc;

use parking_lot::Mutex;

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
        self.state.lock().bodies.push(bytes.to_vec());
        Ok(self.control)
    }

    fn reset_for_attempt(&mut self, _budget: &mut ObserverBudget) -> Result<(), ObserverError> {
        self.state.lock().resets += 1;
        Ok(())
    }

    fn finalize(&mut self, terminal: ObserverTerminal, _budget: &mut ObserverBudget) -> Result<(), ObserverError> {
        self.state.lock().finalizes.push(terminal);
        Ok(())
    }
}

impl Drop for TestObserver {
    fn drop(&mut self) {
        self.state.lock().drops += 1;
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
    let state = state.lock();
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
    let state = state.lock();
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
    assert!(state.lock().bodies.is_empty());
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
        assert_eq!(state.lock().bodies.len(), 1);
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
    assert_eq!(state.lock().bodies, [b"method".to_vec(), b"ignored".to_vec()]);
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
    assert_eq!(healthy_state.lock().bodies, [b"unchanged".to_vec()]);
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
