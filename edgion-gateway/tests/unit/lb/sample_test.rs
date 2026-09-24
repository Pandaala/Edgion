use super::*;
use crate::lb::ewma::score::DEFAULT_RTT_US;
use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_load_balancing::Backend;
use std::net::SocketAddr as StdSocketAddr;

fn backend() -> LbBackend {
    LbBackend::new(Backend {
        addr: SocketAddr::Inet(StdSocketAddr::from(([127, 0, 0, 1], 8080))),
        weight: 1,
        ext: Default::default(),
    })
}

#[test]
fn success_is_useful_work() {
    assert!(is_useful_work(200, None));
    assert!(is_useful_work(304, None));
    assert!(is_useful_work(404, None), "an ordinary 4xx is real traffic");
    assert!(
        is_useful_work(429, None),
        "429 is useful work only when not retryable here"
    );
}

#[test]
fn server_errors_and_retryable_statuses_are_not_useful_work() {
    assert!(!is_useful_work(500, None));
    assert!(!is_useful_work(503, None));
    assert!(
        !is_useful_work(429, Some(&[429])),
        "a retryable 429 must never lower the score"
    );
    assert!(!is_useful_work(404, Some(&[404, 429])));
    assert!(
        is_useful_work(404, Some(&[429])),
        "a status not listed stays useful work"
    );
}

#[test]
fn fast_error_does_not_lower_the_score() {
    // The black hole this classification exists to prevent: a crash-looping
    // pod answering in 2 ms must not acquire the best score in the pool.
    let b = backend();
    assert!(!record_response_sample(&b, 503, None, 2_000, 0));
    assert_eq!(
        b.latency_estimate(0),
        DEFAULT_RTT_US,
        "a fast 5xx must not move the score"
    );
}

#[test]
fn fast_retryable_429_does_not_lower_the_score() {
    // Same hazard one status class down. `retry.codes` is validated to
    // 400-599, so this is a supported configuration, and before this
    // classification a retried 429 recorded the best possible sample.
    let b = backend();
    assert!(!record_response_sample(&b, 429, Some(&[429]), 2_000, 0));
    assert_eq!(b.latency_estimate(0), DEFAULT_RTT_US);
}

#[test]
fn slow_error_still_raises_the_score() {
    // Defect 4's regression guard: peak-only must still penalize slowness.
    let b = backend();
    assert!(record_response_sample(&b, 503, None, 8_000_000, 0));
    assert_eq!(b.latency_estimate(0), 8_000_000);
}

#[test]
fn fast_success_does_lower_the_score() {
    // The contrast case — catches an over-broad predicate that would stop
    // healthy fast backends from ever improving.
    let b = backend();
    b.observe_latency(500_000, 0);
    let before = b.latency_estimate(600);
    assert!(record_response_sample(&b, 200, Some(&[503]), 1_000, 600));
    assert!(
        b.latency_estimate(600) < before,
        "a fast success must pull the estimate down"
    );
}

#[test]
fn error_sample_is_clamped() {
    let b = backend();
    record_response_sample(&b, 503, None, u64::MAX, 0);
    assert_eq!(
        b.latency_estimate(0) as u64,
        MAX_ERROR_SAMPLE_US,
        "an unbounded error sample must not exceed the ratified ceiling"
    );
}

#[test]
fn useful_work_is_not_clamped_by_the_error_ceiling() {
    // A genuinely slow success is a real measurement; only synthetic error
    // samples carry the ceiling.
    let b = backend();
    record_response_sample(&b, 200, None, 400_000_000, 0);
    assert!(b.latency_estimate(0) as u64 > MAX_ERROR_SAMPLE_US);
}
