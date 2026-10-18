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

    let total: u128 = weights.iter().map(|&weight| weight as u128).sum();
    if total > MAX_RR_WEIGHTED_LEN as u128 {
        for w in weights.iter_mut() {
            let scaled = ((*w as u128 * MAX_RR_WEIGHTED_LEN as u128) / total).max(1);
            *w = scaled.min(MAX_RR_WEIGHTED_LEN as u128) as usize;
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

    /// Borrow the next healthy backend from this selector. Returns `None` if all backends are
    /// rejected after `max_iterations` attempts.
    pub fn select(&self, max_iterations: usize, health_filter: impl Fn(&Backend) -> bool) -> Option<&Backend> {
        if self.weighted.is_empty() {
            return None;
        }

        let start = self.counter.fetch_add(1, Ordering::Relaxed);
        let wlen = self.weighted.len();
        let blen = self.backends.len();

        // First try: weighted pick
        let idx = self.weighted[start % wlen] as usize;
        if health_filter(&self.backends[idx]) {
            return Some(&self.backends[idx]);
        }

        // Fallback: walk unique backends starting from the actual weighted pick.
        let offset = idx;
        for step in 1..max_iterations.min(blen) {
            let i = (offset + step) % blen;
            if health_filter(&self.backends[i]) {
                return Some(&self.backends[i]);
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
#[path = "../../../tests/unit/lb/selection/round_robin_test.rs"]
mod tests;
