use pingora_load_balancing::Backend;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Once;

use crate::observe::metrics::registry::record_rr_selector_truncated;

/// Emitted once per process on the first u16::MAX truncation. Subsequent events
/// are only visible via the `edgion_rr_selector_truncated_total` counter.
static TRUNCATION_WARN_ONCE: Once = Once::new();

/// Upper bound on the expanded `weighted` index array length.
///
/// `weighted` is a `Box<[u16]>` (~2 bytes/entry) whose length is the sum of the
/// (normalized) per-backend weights. Per-endpoint weights are hard-coded to 1
/// today, so the sum equals the backend count (<= u16::MAX) and this cap is a
/// no-op; it is defense-in-depth for the day per-endpoint weights are
/// introduced, when a single unclamped `weight` would otherwise expand into a
/// multi-GB allocation. Mirrors the WeightedRoundRobin cap.
const MAX_RR_WEIGHTED_LEN: usize = 1 << 20;

/// Greatest common divisor (Euclid's algorithm). `gcd(x, 0) == x`.
fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}

/// Normalize per-backend weights (each already `>= 1`) so the expanded index
/// array stays within `MAX_RR_WEIGHTED_LEN`, preserving the traffic ratio.
/// Mirrors `weighted_selector::normalize_weights`:
///
/// 1. Reduce by the greatest common divisor — lossless, collapses e.g.
///    `[100, 100]` to `[1, 1]` so common-factor weights cost almost nothing
///    regardless of magnitude.
/// 2. If the total still exceeds the cap (only reachable with adversarial
///    coprime large weights), scale every weight down proportionally, keeping
///    at least 1 so no backend drops out of the rotation.
fn normalize_rr_weights(mut weights: Vec<usize>) -> Vec<usize> {
    let divisor = weights.iter().copied().reduce(gcd).unwrap_or(1).max(1);
    if divisor > 1 {
        for w in weights.iter_mut() {
            *w /= divisor;
        }
    }

    let total: usize = weights.iter().sum();
    if total > MAX_RR_WEIGHTED_LEN {
        for w in weights.iter_mut() {
            *w = ((*w as u128 * MAX_RR_WEIGHTED_LEN as u128) / total as u128).max(1) as usize;
        }
    }

    weights
}

/// Weighted round-robin selector.
///
/// Builds a `weighted` index array where each backend appears `weight` times.
/// An atomic counter advances on every `select()` call, distributing traffic
/// proportionally. Fallback walks the unique backend list on health rejection.
pub struct RoundRobinSelector {
    backends: Box<[Backend]>,
    weighted: Box<[u16]>,
    counter: AtomicUsize,
}

impl RoundRobinSelector {
    /// Build from a slice of backends. Weight < 1 is treated as 1.
    pub fn build(backends: &[Backend]) -> Self {
        let backends = if backends.len() > u16::MAX as usize {
            let total = backends.len();
            record_rr_selector_truncated();
            TRUNCATION_WARN_ONCE.call_once(|| {
                tracing::warn!(
                    count = total,
                    max = u16::MAX,
                    "RoundRobinSelector: backend count exceeds u16::MAX, truncating (subsequent \
                     events only increment metric edgion_rr_selector_truncated_total)"
                );
            });
            &backends[..u16::MAX as usize]
        } else {
            backends
        };
        let mut sorted = backends.to_vec();
        sorted.sort_by(|a, b| a.addr.cmp(&b.addr));

        let weights = normalize_rr_weights(sorted.iter().map(|b| b.weight.max(1)).collect());
        let total_len: usize = weights.iter().sum();
        let mut weighted = Vec::with_capacity(total_len);
        for (index, &w) in weights.iter().enumerate() {
            for _ in 0..w {
                weighted.push(index as u16);
            }
        }

        RoundRobinSelector {
            backends: sorted.into_boxed_slice(),
            weighted: weighted.into_boxed_slice(),
            counter: AtomicUsize::new(0),
        }
    }

    /// Select the next healthy backend. Returns `None` if all backends are
    /// rejected after `max_iterations` attempts.
    pub fn select(&self, max_iterations: usize, health_filter: impl Fn(&Backend) -> bool) -> Option<Backend> {
        if self.weighted.is_empty() {
            return None;
        }

        let start = self.counter.fetch_add(1, Ordering::Relaxed);
        let wlen = self.weighted.len();
        let blen = self.backends.len();

        // First try: weighted pick
        let idx = self.weighted[start % wlen] as usize;
        if health_filter(&self.backends[idx]) {
            return Some(self.backends[idx].clone());
        }

        // Fallback: walk unique backends starting from weighted position
        let offset = start % blen;
        for step in 1..max_iterations.min(blen) {
            let i = (offset + step) % blen;
            if health_filter(&self.backends[i]) {
                return Some(self.backends[i].clone());
            }
        }

        None
    }

    /// Number of unique backends.
    pub fn backend_count(&self) -> usize {
        self.backends.len()
    }

    /// Access the sorted backend list.
    pub fn backends(&self) -> &[Backend] {
        &self.backends
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrics_exporter_prometheus::PrometheusBuilder;
    use pingora_core::protocols::l4::socket::SocketAddr as PSocketAddr;
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn make_backend(i: u32) -> Backend {
        let octets = i.to_be_bytes();
        let ip = IpAddr::V4(Ipv4Addr::new(10, octets[1], octets[2], octets[3]));
        Backend {
            addr: PSocketAddr::Inet(SocketAddr::new(ip, 80)),
            weight: 1,
            ext: Default::default(),
        }
    }

    fn make_backend_weighted(i: u32, weight: usize) -> Backend {
        let mut b = make_backend(i);
        b.weight = weight;
        b
    }

    #[test]
    fn build_truncates_over_u16_max() {
        let backends: Vec<Backend> = (0..(u16::MAX as u32 + 2)).map(make_backend).collect();
        let selector = RoundRobinSelector::build(&backends);
        assert_eq!(selector.backend_count(), u16::MAX as usize);
    }

    #[test]
    fn truncation_increments_counter() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            let backends: Vec<Backend> = (0..(u16::MAX as u32 + 1)).map(make_backend).collect();
            let _ = RoundRobinSelector::build(&backends);
            let rendered = handle.render();
            assert!(
                rendered.contains("edgion_rr_selector_truncated_total"),
                "counter must appear on truncation, got:\n{rendered}"
            );
        });
    }

    #[test]
    fn empty_backend_list_returns_none() {
        let selector = RoundRobinSelector::build(&[]);
        assert!(selector.select(8, |_| true).is_none());
        assert_eq!(selector.backend_count(), 0);
    }

    #[test]
    fn single_backend_returns_it_when_healthy() {
        let backends = vec![make_backend(1)];
        let selector = RoundRobinSelector::build(&backends);
        for _ in 0..5 {
            let picked = selector.select(8, |_| true).expect("healthy backend");
            assert_eq!(picked.addr, backends[0].addr);
        }
    }

    #[test]
    fn single_backend_returns_none_when_unhealthy() {
        let backends = vec![make_backend(1)];
        let selector = RoundRobinSelector::build(&backends);
        assert!(selector.select(8, |_| false).is_none());
    }

    #[test]
    fn distribution_uniform_for_equal_weights() {
        // N equal-weight backends, N consecutive selections must touch each
        // backend exactly once thanks to the deterministic counter.
        let n = 5u32;
        let backends: Vec<Backend> = (1..=n).map(make_backend).collect();
        let selector = RoundRobinSelector::build(&backends);

        let mut counts: HashMap<PSocketAddr, usize> = HashMap::new();
        for _ in 0..n {
            let picked = selector.select(8, |_| true).expect("healthy backend");
            *counts.entry(picked.addr).or_default() += 1;
        }
        assert_eq!(counts.len(), n as usize, "each backend must be picked once");
        for (_, c) in counts {
            assert_eq!(c, 1);
        }
    }

    #[test]
    fn distribution_respects_weights() {
        // Weights 1:2:5 => over 8 calls each backend appears `weight` times.
        let backends = vec![
            make_backend_weighted(1, 1),
            make_backend_weighted(2, 2),
            make_backend_weighted(3, 5),
        ];
        let selector = RoundRobinSelector::build(&backends);

        let total: usize = backends.iter().map(|b| b.weight).sum();
        let mut counts: HashMap<PSocketAddr, usize> = HashMap::new();
        for _ in 0..total {
            let picked = selector.select(8, |_| true).expect("healthy backend");
            *counts.entry(picked.addr).or_default() += 1;
        }
        // The selector sorts by addr; addresses 10.0.0.1/2/3 keep order.
        assert_eq!(counts.get(&backends[0].addr).copied().unwrap_or(0), 1);
        assert_eq!(counts.get(&backends[1].addr).copied().unwrap_or(0), 2);
        assert_eq!(counts.get(&backends[2].addr).copied().unwrap_or(0), 5);
    }

    #[test]
    fn zero_weight_treated_as_one() {
        // weight = 0 should be normalized to 1, so the backend still appears.
        let backends = vec![make_backend_weighted(1, 0), make_backend_weighted(2, 0)];
        let selector = RoundRobinSelector::build(&backends);

        let mut seen: HashMap<PSocketAddr, usize> = HashMap::new();
        for _ in 0..4 {
            let picked = selector.select(8, |_| true).expect("healthy backend");
            *seen.entry(picked.addr).or_default() += 1;
        }
        assert_eq!(seen.len(), 2, "both zero-weight backends must rotate in");
    }

    #[test]
    fn all_unhealthy_returns_none() {
        let backends: Vec<Backend> = (1..=4).map(make_backend).collect();
        let selector = RoundRobinSelector::build(&backends);
        assert!(selector.select(16, |_| false).is_none());
    }

    #[test]
    fn fallback_walks_to_lone_healthy_backend() {
        let backends: Vec<Backend> = (1..=8).map(make_backend).collect();
        let selector = RoundRobinSelector::build(&backends);
        let healthy = backends[5].addr.clone();
        let picked = selector
            .select(16, |b| b.addr == healthy)
            .expect("must locate the lone healthy backend");
        assert_eq!(picked.addr, healthy);
    }

    #[test]
    fn max_iterations_caps_fallback_search() {
        // With max_iterations = 1 the fallback loop body never runs (the
        // condition is `1..max_iterations.min(blen)` which is empty), so the
        // lone healthy backend must NOT be reached when the first weighted
        // pick is unhealthy.
        let backends: Vec<Backend> = (1..=4).map(make_backend).collect();
        let selector = RoundRobinSelector::build(&backends);
        let healthy = backends[2].addr.clone();
        // Force the first counter pick to land on a non-healthy backend by
        // exhausting until counter % 4 == 0 (backends[0]).
        // counter starts at 0 → idx 0 (unhealthy). Fallback does nothing.
        let picked = selector.select(1, |b| b.addr == healthy);
        assert!(
            picked.is_none(),
            "max_iterations=1 must skip fallback walk, got {:?}",
            picked
        );
    }

    #[test]
    fn huge_per_backend_weight_is_capped_and_light_backend_survives() {
        // Defense-in-depth: an adversarially large per-backend weight must not
        // expand the `weighted` index array without bound. With the cap, build
        // must not OOM and every backend must keep at least one slot — proven by
        // the fallback walk still locating the light backend when the heavy one
        // is rejected. (Today all weights are 1; this guards the future.)
        let backends = vec![make_backend_weighted(1, 100_000_000), make_backend_weighted(2, 1)];
        let light = backends[1].addr.clone();
        let selector = RoundRobinSelector::build(&backends);
        let picked = selector
            .select(8, |b| b.addr == light)
            .expect("light backend must remain reachable after weight capping");
        assert_eq!(picked.addr, light);
    }

    #[test]
    fn common_factor_weights_are_reduced() {
        // GCD reduction: weights [100, 100] collapse to [1, 1], so the index
        // array length is 2 (not 200) and two consecutive selections touch each
        // backend once. Without the reduction both selections would land on the
        // first backend's run of duplicate slots.
        let backends = vec![make_backend_weighted(1, 100), make_backend_weighted(2, 100)];
        let selector = RoundRobinSelector::build(&backends);
        let mut counts: HashMap<PSocketAddr, usize> = HashMap::new();
        for _ in 0..2 {
            let picked = selector.select(8, |_| true).expect("healthy backend");
            *counts.entry(picked.addr).or_default() += 1;
        }
        assert_eq!(
            counts.len(),
            2,
            "equal weights must round-robin one-each regardless of magnitude"
        );
    }

    #[test]
    fn backends_accessor_returns_sorted_unique_list() {
        // Build with shuffled input; ensure backends() is sorted by addr.
        let mut backends: Vec<Backend> = (1..=4).map(make_backend).collect();
        backends.swap(0, 3);
        backends.swap(1, 2);
        let selector = RoundRobinSelector::build(&backends);
        let stored = selector.backends();
        assert_eq!(stored.len(), 4);
        for w in stored.windows(2) {
            assert!(w[0].addr <= w[1].addr, "backends must be sorted by addr");
        }
    }
}
