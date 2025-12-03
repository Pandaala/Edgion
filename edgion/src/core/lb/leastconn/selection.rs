//! LeastConnection backend selection implementation
//!
//! Implements Pingora's BackendSelection trait for least-connection load balancing.

use pingora_load_balancing::selection::{BackendIter, BackendSelection};
use pingora_load_balancing::Backend;
use std::collections::BTreeSet;
use std::sync::Arc;

use super::counter;

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
pub struct LeastConnectionIter {
    backend: Arc<LeastConnection>,
    index: usize,
    sorted_indices: Vec<usize>,
}

impl BackendSelection for LeastConnection {
    type Iter = LeastConnectionIter;

    fn build(backends: &BTreeSet<Backend>) -> Self {
        Self {
            backends: Vec::from_iter(backends.iter().cloned()).into_boxed_slice(),
        }
    }

    fn iter(self: &Arc<Self>, _key: &[u8]) -> Self::Iter {
        // Sort backends by connection count (ascending)
        let mut indices: Vec<usize> = (0..self.backends.len()).collect();
        indices.sort_by_key(|&i| counter::get_count(&self.backends[i].addr));

        LeastConnectionIter {
            backend: self.clone(),
            index: 0,
            sorted_indices: indices,
        }
    }
}

impl BackendIter for LeastConnectionIter {
    fn next(&mut self) -> Option<&Backend> {
        if self.index >= self.sorted_indices.len() {
            return None;
        }
        let backend_idx = self.sorted_indices[self.index];
        self.index += 1;
        Some(&self.backend.backends[backend_idx])
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
}

