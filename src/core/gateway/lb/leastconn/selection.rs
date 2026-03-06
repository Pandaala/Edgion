//! LeastConnection backend selection implementation
//!
//! Implements Pingora's BackendSelection trait for least-connection load balancing.

use pingora_load_balancing::selection::{BackendIter, BackendSelection};
use pingora_load_balancing::Backend;
use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap};
use std::sync::Arc;

use super::{backend_state, counter};

/// LeastConnection backend selection
///
/// Selects backends with the fewest active connections.
/// Uses the global connection counter to track active connections.
pub struct LeastConnection {
    backends: Box<[Backend]>,
}

/// Iterator for LeastConnection selection
///
/// Returns backends in order of least connections first.
/// Uses a min-heap for efficient selection.
pub struct LeastConnectionIter {
    backend: Arc<LeastConnection>,
    heap: BinaryHeap<Reverse<(usize, usize)>>,
}

impl BackendSelection for LeastConnection {
    type Iter = LeastConnectionIter;
    type Config = ();

    fn build(backends: &BTreeSet<Backend>) -> Self {
        Self {
            backends: Vec::from_iter(backends.iter().cloned()).into_boxed_slice(),
        }
    }

    fn iter(self: &Arc<Self>, _key: &[u8]) -> Self::Iter {
        // Build min-heap with only active backends
        // Heap sorts by (connection_count, backend_index) in ascending order
        let mut heap = BinaryHeap::new();

        for (i, backend) in self.backends.iter().enumerate() {
            // Skip non-active backends (draining or removed)
            if !backend_state::is_active(&backend.addr) {
                continue;
            }

            let count = counter::get_count(&backend.addr);
            heap.push(Reverse((count, i)));
        }

        LeastConnectionIter {
            backend: self.clone(),
            heap,
        }
    }
}

impl BackendIter for LeastConnectionIter {
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
        let lc = LeastConnection::build(&backends);
        assert!(lc.backends.is_empty());
    }

    #[test]
    fn test_build_with_backends() {
        let mut backends = BTreeSet::new();
        backends.insert(Backend::new("127.0.0.1:8080").unwrap());
        backends.insert(Backend::new("127.0.0.1:8081").unwrap());

        let lc = LeastConnection::build(&backends);
        assert_eq!(lc.backends.len(), 2);
    }

    #[test]
    fn test_iter_selects_least_connections() {
        let mut backends = BTreeSet::new();
        let b1 = Backend::new("127.0.0.1:18080").unwrap();
        let b2 = Backend::new("127.0.0.1:18081").unwrap();
        backends.insert(b1.clone());
        backends.insert(b2.clone());

        let lc = Arc::new(LeastConnection::build(&backends));

        // Simulate connections: b1 has 2, b2 has 0
        counter::increment(&b1.addr);
        counter::increment(&b1.addr);

        let mut iter = lc.iter(b"test");

        // First should be b2 (0 connections)
        let first = iter.next().unwrap();
        assert_eq!(first.addr, b2.addr);

        // Second should be b1 (2 connections)
        let second = iter.next().unwrap();
        assert_eq!(second.addr, b1.addr);

        // No more
        assert!(iter.next().is_none());

        // Cleanup
        counter::decrement(&b1.addr);
        counter::decrement(&b1.addr);
    }

    #[test]
    fn test_connection_counting_affects_selection() {
        let mut backends = BTreeSet::new();
        let b1 = Backend::new("127.0.0.1:28080").unwrap();
        let b2 = Backend::new("127.0.0.1:28081").unwrap();
        backends.insert(b1.clone());
        backends.insert(b2.clone());

        let lc = Arc::new(LeastConnection::build(&backends));

        // Simulate b1 has 2 connections
        counter::increment(&b1.addr);
        counter::increment(&b1.addr);

        // Should select b2 (0 connections)
        let mut iter = lc.iter(b"test");
        let first = iter.next().unwrap();
        assert_eq!(first.addr, b2.addr);

        // Cleanup
        counter::decrement(&b1.addr);
        counter::decrement(&b1.addr);
    }

    #[test]
    fn test_draining_backend_not_selected() {
        let mut backends = BTreeSet::new();
        let b1 = Backend::new("127.0.0.1:38080").unwrap();
        let b2 = Backend::new("127.0.0.1:38081").unwrap();
        backends.insert(b1.clone());
        backends.insert(b2.clone());

        let lc = Arc::new(LeastConnection::build(&backends));

        // Mark b1 as draining
        backend_state::mark_draining(&b1.addr);

        // Should only select b2
        let mut iter = lc.iter(b"test");
        let first = iter.next().unwrap();
        assert_eq!(first.addr, b2.addr);

        let second = iter.next();
        assert!(second.is_none(), "Should not select draining backend");

        // Cleanup
        backend_state::remove(&b1.addr);
    }

    #[test]
    fn test_reactivate_inherits_count() {
        let b1 = Backend::new("127.0.0.1:48080").unwrap();

        // Simulate connections
        counter::increment(&b1.addr);
        counter::increment(&b1.addr);
        assert_eq!(counter::get_count(&b1.addr), 2);

        // Mark as draining
        backend_state::mark_draining(&b1.addr);

        // Reactivate
        backend_state::reactivate(&b1.addr);

        // Should inherit count
        assert_eq!(counter::get_count(&b1.addr), 2);
        assert!(backend_state::is_active(&b1.addr));

        // Cleanup
        counter::decrement(&b1.addr);
        counter::decrement(&b1.addr);
    }

    #[test]
    fn test_heap_ordering_with_multiple_backends() {
        let mut backends = BTreeSet::new();
        let b1 = Backend::new("127.0.0.1:58080").unwrap();
        let b2 = Backend::new("127.0.0.1:58081").unwrap();
        let b3 = Backend::new("127.0.0.1:58082").unwrap();
        backends.insert(b1.clone());
        backends.insert(b2.clone());
        backends.insert(b3.clone());

        // Set: b1=1, b2=2, b3=3
        counter::increment(&b1.addr);
        counter::increment(&b2.addr);
        counter::increment(&b2.addr);
        counter::increment(&b3.addr);
        counter::increment(&b3.addr);
        counter::increment(&b3.addr);

        let lc = Arc::new(LeastConnection::build(&backends));
        let mut iter = lc.iter(b"test");

        assert_eq!(iter.next().unwrap().addr, b1.addr);
        assert_eq!(iter.next().unwrap().addr, b2.addr);
        assert_eq!(iter.next().unwrap().addr, b3.addr);

        // Cleanup
        counter::decrement(&b1.addr);
        counter::decrement(&b2.addr);
        counter::decrement(&b2.addr);
        counter::decrement(&b3.addr);
        counter::decrement(&b3.addr);
        counter::decrement(&b3.addr);
    }
}
