use super::*;

#[test]
fn aggregate_permit_is_bounded_and_released() {
    let state = LogicalBackendCircuitBreaker::new(1);
    let permit = Arc::clone(&state).try_acquire().unwrap();
    assert!(Arc::clone(&state).try_acquire().is_none());
    assert_eq!(state.in_flight(), 1);
    drop(permit);
    assert_eq!(state.in_flight(), 0);
    assert!(Arc::clone(&state).try_acquire().is_some());
}

#[test]
fn owned_permit_retains_exact_generation_until_drop() {
    let state = LogicalBackendCircuitBreaker::new(1);
    let address = Arc::as_ptr(&state);
    let weak = Arc::downgrade(&state);
    let permit = state.try_acquire().unwrap();
    assert_eq!(Arc::as_ptr(&permit.state), address);
    assert_eq!(weak.strong_count(), 1);
    assert_eq!(permit.state.in_flight(), 1);
    drop(permit);
    assert!(weak.upgrade().is_none());

    let disabled = LogicalBackendCircuitBreaker::new(0);
    let weak = Arc::downgrade(&disabled);
    assert!(disabled.try_acquire().is_none());
    assert!(weak.upgrade().is_none());
}

#[test]
fn maximum_limit_does_not_overflow_and_release_restores_capacity() {
    let state = LogicalBackendCircuitBreaker::new(u32::MAX);
    state.in_flight.store(u32::MAX - 1, Ordering::Release);
    let permit = Arc::clone(&state).try_acquire().unwrap();
    assert_eq!(state.in_flight(), u32::MAX);
    assert!(Arc::clone(&state).try_acquire().is_none());
    assert_eq!(state.in_flight(), u32::MAX);
    drop(permit);
    assert_eq!(state.in_flight(), u32::MAX - 1);
    assert!(Arc::clone(&state).try_acquire().is_some());
    assert_eq!(state.in_flight(), u32::MAX - 1);
}
