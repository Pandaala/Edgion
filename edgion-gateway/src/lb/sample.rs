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
#[path = "../../tests/unit/lb/sample_test.rs"]
mod tests;
