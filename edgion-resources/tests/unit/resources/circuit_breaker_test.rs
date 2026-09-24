use super::*;
#[test]
fn zero_max_parallel_requests_rejected() {
    let c = CircuitBreakerConfig {
        max_parallel_requests: 0,
    };
    assert!(c.get_validation_error().is_some());
}
#[test]
fn positive_max_parallel_requests_ok() {
    let c = CircuitBreakerConfig {
        max_parallel_requests: 1024,
    };
    assert!(c.get_validation_error().is_none());
}
