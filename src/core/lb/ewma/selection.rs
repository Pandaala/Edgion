//! EWMA backend selection implementation
//!
//! Implements Pingora's BackendSelection trait for EWMA-based load balancing.
//! Selects backends with the lowest EWMA response time.

use pingora_load_balancing::selection::{BackendIter, BackendSelection};
use pingora_load_balancing::Backend;
use std::collections::{BTreeSet, BinaryHeap};
use std::cmp::Reverse;
use std::sync::Arc;

use super::metrics;
use crate::core::lb::leastconn::backend_state;

/// EWMA backend selection
///
/// Selects backends with the lowest EWMA response time.
/// Uses the global EWMA metrics to track response latency.
pub struct Ewma {
    backends: Box<[Backend]>,
}

/// Iterator for EWMA selection
///
/// Returns backends in order of lowest EWMA first.
/// Uses a min-heap for efficient selection.
pub struct EwmaIter {
    backend: Arc<Ewma>,
    heap: BinaryHeap<Reverse<(u64, usize)>>,
}

impl BackendSelection for Ewma {
    type Iter = EwmaIter;

    fn build(backends: &BTreeSet<Backend>) -> Self {
        Self {
            backends: Vec::from_iter(backends.iter().cloned()).into_boxed_slice(),
        }
    }

    #[inline]
    fn iter(self: &Arc<Self>, _key: &[u8]) -> Self::Iter {
        // Build min-heap with only active backends
        // Heap sorts by (ewma_value, backend_index) in ascending order
        // Performance: O(n) for heap construction where n = number of active backends
        let mut heap = BinaryHeap::with_capacity(self.backends.len());
        
        for (i, backend) in self.backends.iter().enumerate() {
            // Skip non-active backends (draining or removed)
            if !backend_state::is_active(&backend.addr) {
                continue;
            }
            
            let ewma = metrics::get_ewma(&backend.addr);
            heap.push(Reverse((ewma, i)));
        }

        EwmaIter {
            backend: self.clone(),
            heap,
        }
    }
}

impl BackendIter for EwmaIter {
    #[inline]
    fn next(&mut self) -> Option<&Backend> {
        self.heap.pop().map(|Reverse((_, idx))| &self.backend.backends[idx])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pingora_load_balancing::Backend;

    #[test]
    fn test_build_empty() {
        let backends = BTreeSet::new();
        let ewma = Ewma::build(&backends);
        assert!(ewma.backends.is_empty());
    }

    #[test]
    fn test_build_with_backends() {
        let mut backends = BTreeSet::new();
        backends.insert(Backend::new("127.0.0.1:8080").unwrap());
        backends.insert(Backend::new("127.0.0.1:8081").unwrap());

        let ewma = Ewma::build(&backends);
        assert_eq!(ewma.backends.len(), 2);
    }

    #[test]
    fn test_iter_selects_lowest_ewma() {
        let mut backends = BTreeSet::new();
        let b1 = Backend::new("127.0.0.1:19080").unwrap();
        let b2 = Backend::new("127.0.0.1:19081").unwrap();
        backends.insert(b1.clone());
        backends.insert(b2.clone());

        let ewma = Arc::new(Ewma::build(&backends));

        // Simulate EWMA: b1 has higher latency (5ms), b2 has lower (2ms)
        metrics::update(&b1.addr, 5_000);
        metrics::update(&b2.addr, 2_000);

        let mut iter = ewma.iter(b"test");

        // First should be b2 (lower EWMA)
        let first = iter.next().unwrap();
        assert_eq!(first.addr, b2.addr);

        // Second should be b1 (higher EWMA)
        let second = iter.next().unwrap();
        assert_eq!(second.addr, b1.addr);

        // No more
        assert!(iter.next().is_none());

        // Cleanup
        metrics::remove(&b1.addr);
        metrics::remove(&b2.addr);
    }

    #[test]
    fn test_draining_backend_not_selected() {
        let mut backends = BTreeSet::new();
        let b1 = Backend::new("127.0.0.1:39080").unwrap();
        let b2 = Backend::new("127.0.0.1:39081").unwrap();
        backends.insert(b1.clone());
        backends.insert(b2.clone());
        
        let ewma = Arc::new(Ewma::build(&backends));
        
        // Mark b1 as draining
        backend_state::mark_draining(&b1.addr);
        
        // Should only select b2
        let mut iter = ewma.iter(b"test");
        let first = iter.next().unwrap();
        assert_eq!(first.addr, b2.addr);
        
        let second = iter.next();
        assert!(second.is_none(), "Should not select draining backend");
        
        // Cleanup
        backend_state::remove(&b1.addr);
    }
    
    #[test]
    fn test_heap_ordering_with_multiple_backends() {
        let mut backends = BTreeSet::new();
        let b1 = Backend::new("127.0.0.1:59080").unwrap();
        let b2 = Backend::new("127.0.0.1:59081").unwrap();
        let b3 = Backend::new("127.0.0.1:59082").unwrap();
        backends.insert(b1.clone());
        backends.insert(b2.clone());
        backends.insert(b3.clone());
        
        // Set EWMA: b1=3ms, b2=1ms, b3=2ms
        metrics::update(&b1.addr, 3_000);
        metrics::update(&b2.addr, 1_000);
        metrics::update(&b3.addr, 2_000);
        
        let ewma = Arc::new(Ewma::build(&backends));
        let mut iter = ewma.iter(b"test");
        
        // Should be ordered by EWMA: b2(1ms) < b3(2ms) < b1(3ms)
        assert_eq!(iter.next().unwrap().addr, b2.addr);
        assert_eq!(iter.next().unwrap().addr, b3.addr);
        assert_eq!(iter.next().unwrap().addr, b1.addr);
        
        // Cleanup
        metrics::remove(&b1.addr);
        metrics::remove(&b2.addr);
        metrics::remove(&b3.addr);
    }

    #[test]
    fn test_new_backend_gets_initial_ewma() {
        let mut backends = BTreeSet::new();
        let b1 = Backend::new("127.0.0.1:29080").unwrap();
        let b2 = Backend::new("127.0.0.1:29081").unwrap();
        backends.insert(b1.clone());
        backends.insert(b2.clone());
        
        // b1 has history (high latency), b2 is new
        metrics::update(&b1.addr, 10_000);
        
        let ewma = Arc::new(Ewma::build(&backends));
        let mut iter = ewma.iter(b"test");
        
        // b2 should be selected first (initial EWMA is low)
        let first = iter.next().unwrap();
        assert_eq!(first.addr, b2.addr);
        
        // Cleanup
        metrics::remove(&b1.addr);
    }
}

