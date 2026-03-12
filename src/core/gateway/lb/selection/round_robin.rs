use pingora_load_balancing::Backend;
use std::sync::atomic::{AtomicUsize, Ordering};

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
        assert!(
            backends.len() <= u16::MAX as usize,
            "RoundRobinSelector supports up to 2^16 backends"
        );
        let mut sorted = backends.to_vec();
        sorted.sort_by(|a, b| a.addr.cmp(&b.addr));

        let mut weighted = Vec::with_capacity(sorted.len());
        for (index, b) in sorted.iter().enumerate() {
            for _ in 0..b.weight.max(1) {
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
    pub fn select(
        &self,
        max_iterations: usize,
        health_filter: impl Fn(&Backend) -> bool,
    ) -> Option<Backend> {
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
