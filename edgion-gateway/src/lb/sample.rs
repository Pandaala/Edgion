//! Classification of an upstream response into a load-balancer latency sample.
//!
//! Extracted from the response-filter hook so the policy is unit-testable: the
//! hook itself is only reachable through a `pingora_proxy` session.
//!
//! Design record: `skills/12-algorithms/lb-peak-ewma.md`.

use crate::lb::backend::LbBackend;

/// Upper bound on a synthetic error sample, in µs (300 s).
///
/// Time-to-headers is bounded by the resolved peer read timeout, but that is
/// operator-configurable — an External-jump `upstream_read_timeout_override`
/// raises it, and `backendRequestTimeout: "0s"` with no route or gRPC deadline
/// leaves reads unbounded. A hung backend must not be able to de-prioritize
/// itself for an unbounded period on the strength of its own slowness, so the
/// error branch shares the ceiling the overload-penalty design ratified for
/// exactly this quantity (`skills/12-algorithms/lb-overload-penalty.md`).
///
/// At a 10 s half-life a 300 s peak still costs ~155 s to return to parity.
pub const MAX_ERROR_SAMPLE_US: u64 = 300_000_000;

/// Does this response's elapsed time measure **useful work**?
///
/// Only a useful-work sample may lower a backend's score through the blend.
/// Everything else is recorded peak-only, so it can penalize a slow backend but
/// can never reward a fast-failing one.
///
/// Two classes are excluded:
///
/// * **Any `5xx`.** A server error's elapsed time is not work delivered.
/// * **Any status this route declares retryable.** A status the operator has
///   listed in `retry.codes` is by definition not a useful outcome — the
///   gateway is going to discard it and try elsewhere. This matters most for
///   `429`: it is the archetypal fast rejection, and `retry.codes` is validated
///   to 400-599 so `codes: [429]` is a first-class configuration. Classifying
///   it as useful work would let a rate-limited or crash-looping backend answer
///   in ~2 ms, acquire the best score in the pool, and win nearly every P2C
///   matchup while every request pays a retry.
///
/// Deliberately **not** reused here: `FailureClass::from_http_status`. That
/// predicate is Envoy-shaped for *ejection* policy, where a 429 counts as a
/// success so a rate-limited host is not evicted. The correct classification
/// for *scoring* is the opposite one, so the two must not share a predicate.
#[inline]
pub fn is_useful_work(status: u16, retry_codes: Option<&[u16]>) -> bool {
    if status >= 500 {
        return false;
    }
    !retry_codes.is_some_and(|codes| codes.contains(&status))
}

/// Fold one upstream response into the backend's Peak-EWMA score.
///
/// Returns `true` if the value was stored. A useful-work sample always stores
/// (peak or blend); a non-useful one stores only when it is a strict peak.
#[inline]
pub fn record_response_sample(
    backend: &LbBackend,
    status: u16,
    retry_codes: Option<&[u16]>,
    latency_us: u64,
    now_units: u32,
) -> bool {
    if is_useful_work(status, retry_codes) {
        backend.observe_latency(latency_us, now_units);
        true
    } else {
        backend.observe_peak_only(latency_us.min(MAX_ERROR_SAMPLE_US), now_units)
    }
}

#[cfg(test)]
mod tests {
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
}
