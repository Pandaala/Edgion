use pingora_load_balancing::Backend;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Once;

use crate::core::gateway::observe::metrics::registry::record_rr_selector_truncated;

/// Emitted once per process on the first u16::MAX truncation. Subsequent events
/// are only visible via the `edgion_rr_selector_truncated_total` counter.
static TRUNCATION_WARN_ONCE: Once = Once::new();

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
}
