use super::*;

#[test]
fn aggregate_permit_is_bounded_and_released() {
    let state = LogicalBackendCircuitBreaker::new(1);
    let permit = state.try_acquire().unwrap();
    assert!(state.try_acquire().is_none());
    assert_eq!(state.in_flight(), 1);
    drop(permit);
    assert_eq!(state.in_flight(), 0);
    assert!(state.try_acquire().is_some());
}
