//! Power of Two Choices (P2C) selection over `Arc<LbBackend>`.
//!
//! Used by LeastConn / EWMA to pick a backend without scanning the full list.
//! Two random backends are sampled; the one with the smaller metric is
//! preferred. If it fails the health filter the other is tried; on repeated
//! rejection the search restarts up to `max_retries` times, then falls back to
//! a linear scan as a last-resort liveness guarantee.
//
// TODO: evaluate strict O(log N) min-metric selection (lazy-deletion heap with
// metric versioning) if production data shows P2C underperforms on highly
// skewed services. P2C is the same approach used by Envoy / Linkerd2 / Pingora;
// it should suffice for the common case.

use crate::lb::backend::LbBackend;
use rand::Rng;
use std::sync::Arc;

/// Map `raw` in `0..n - 1` to an index in `0..n` that is never `excluded`.
/// Shared with the AI adaptive picker so both P2C users sample distinct pairs.
#[inline]
pub(crate) fn skip_excluded_index(raw: usize, excluded: usize) -> usize {
    if raw >= excluded {
        raw + 1
    } else {
        raw
    }
}

/// Pick a backend via P2C.
///
/// * `backends` — current snapshot list. Empty list returns `None`.
/// * `metric` — function returning the comparable metric (lower is better)
///   for a backend. Must be cheap (typically an atomic load).
/// * `health_filter` — predicate combining active health checks and circuit
///   breaker state. Rejected picks fall through to the secondary candidate.
/// * `max_retries` — caps the worst-case retry budget when both samples fail
///   the health filter, bounding latency on degraded services.
pub fn select_p2c<F, M>(
    backends: &[Arc<LbBackend>],
    metric: M,
    health_filter: F,
    max_retries: usize,
) -> Option<Arc<LbBackend>>
where
    F: Fn(&LbBackend) -> bool,
    M: Fn(&LbBackend) -> u64,
{
    if backends.is_empty() {
        return None;
    }
    if backends.len() == 1 {
        let b = &backends[0];
        return health_filter(b).then(|| b.clone());
    }

    let n = backends.len();
    let mut rng = rand::rng();
    for _ in 0..max_retries {
        let i = rng.random_range(0..n);
        // For n=2 the other index is always deterministic; avoid the retry loop.
        let j = if n == 2 {
            1 - i
        } else {
            let raw = rng.random_range(0..n - 1);
            skip_excluded_index(raw, i)
        };
        let a = &backends[i];
        let b = &backends[j];
        let (pick, other) = if metric(a) <= metric(b) { (a, b) } else { (b, a) };
        if health_filter(pick) {
            return Some(pick.clone());
        }
        if health_filter(other) {
            return Some(other.clone());
        }
    }
    // Last-resort: linear scan so a healthy needle in a mostly-unhealthy
    // haystack is still found.
    backends.iter().find(|b| health_filter(b)).cloned()
}

#[cfg(test)]
#[path = "../../../tests/unit/lb/selection/p2c_test.rs"]
mod tests;
