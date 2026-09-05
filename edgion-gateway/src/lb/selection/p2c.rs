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
            let mut j = rng.random_range(0..n);
            while j == i {
                j = rng.random_range(0..n);
            }
            j
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
mod tests {
    use super::*;
    use pingora_core::protocols::l4::socket::SocketAddr as PSocketAddr;
    use pingora_load_balancing::Backend;
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn backend(i: u32) -> Arc<LbBackend> {
        let octets = i.to_be_bytes();
        let ip = IpAddr::V4(Ipv4Addr::new(10, octets[1], octets[2], octets[3]));
        Arc::new(LbBackend::new(Backend {
            addr: PSocketAddr::Inet(SocketAddr::new(ip, 80)),
            weight: 1,
            ext: Default::default(),
        }))
    }

    #[test]
    fn empty_list_returns_none() {
        let result = select_p2c::<_, _>(&[], |_| 0u64, |_| true, 4);
        assert!(result.is_none());
    }

    #[test]
    fn single_backend_returns_it_when_healthy() {
        let bs = vec![backend(1)];
        let picked = select_p2c(&bs, |_| 0u64, |_| true, 4);
        assert!(picked.is_some());
    }

    #[test]
    fn single_backend_returns_none_when_unhealthy() {
        let bs = vec![backend(1)];
        let picked = select_p2c(&bs, |_| 0u64, |_| false, 4);
        assert!(picked.is_none());
    }

    #[test]
    fn fallback_finds_lone_healthy_backend_among_unhealthy() {
        let bs: Vec<Arc<LbBackend>> = (0..16).map(backend).collect();
        let healthy_addr = bs[7].inner().addr.clone();
        let picked = select_p2c(&bs, |_| 0u64, |b| b.inner().addr == healthy_addr, 4);
        assert!(picked.is_some());
        assert_eq!(picked.unwrap().inner().addr, healthy_addr);
    }

    /// P2C distribution sanity check: with uniform metrics, no backend should
    /// be picked dramatically more or less than its fair share. This is a
    /// regression check for the "two-different-indices" sampling, not a
    /// statistical guarantee.
    #[test]
    fn p2c_distribution_within_tolerance_under_uniform_metric() {
        let n = 32usize;
        let bs: Vec<Arc<LbBackend>> = (0..n as u32).map(backend).collect();
        let trials = 10_000usize;
        let mut counts: HashMap<std::net::SocketAddr, usize> = HashMap::new();
        for _ in 0..trials {
            let picked = select_p2c(&bs, |_| 0u64, |_| true, 4).expect("has healthy backend");
            if let Some(addr) = picked.inner().addr.as_inet() {
                *counts.entry(*addr).or_default() += 1;
            }
        }
        let expected = trials / n;
        let lower = expected * 60 / 100; // 60% of fair share
        let upper = expected * 160 / 100; // 160% of fair share
        for (addr, count) in &counts {
            assert!(
                *count >= lower && *count <= upper,
                "backend {} got {} picks, expected {}±~40% (range {}..={})",
                addr,
                count,
                expected,
                lower,
                upper
            );
        }
    }
}
