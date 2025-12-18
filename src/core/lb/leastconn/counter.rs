//! Global connection counter for LeastConnection load balancing
//!
//! Tracks active connections per backend address using a thread-safe DashMap.

use dashmap::DashMap;
use std::sync::LazyLock;
use pingora_core::protocols::l4::socket::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Global connection counts per backend address
static CONNECTION_COUNTS: LazyLock<DashMap<SocketAddr, AtomicUsize>> = LazyLock::new(DashMap::new);

/// Increment the connection count for a backend address.
/// Call this when a new connection is established.
pub fn increment(addr: &SocketAddr) {
    CONNECTION_COUNTS
        .entry(addr.clone())
        .or_insert_with(|| AtomicUsize::new(0))
        .fetch_add(1, Ordering::Relaxed);
}

/// Decrement the connection count for a backend address.
/// Call this when a connection is closed.
pub fn decrement(addr: &SocketAddr) {
    if let Some(count) = CONNECTION_COUNTS.get(addr) {
        // Use saturating_sub to avoid underflow
        let current = count.load(Ordering::Relaxed);
        if current > 0 {
            count.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// Get the current connection count for a backend address.
pub fn get_count(addr: &SocketAddr) -> usize {
    CONNECTION_COUNTS
        .get(addr)
        .map(|c| c.load(Ordering::Relaxed))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr as StdSocketAddr;

    fn make_addr(port: u16) -> SocketAddr {
        let std_addr: StdSocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
        SocketAddr::Inet(std_addr)
    }

    #[test]
    fn test_increment_decrement() {
        let addr = make_addr(9999);
        
        assert_eq!(get_count(&addr), 0);
        
        increment(&addr);
        assert_eq!(get_count(&addr), 1);
        
        increment(&addr);
        assert_eq!(get_count(&addr), 2);
        
        decrement(&addr);
        assert_eq!(get_count(&addr), 1);
        
        decrement(&addr);
        assert_eq!(get_count(&addr), 0);
        
        // Should not underflow
        decrement(&addr);
        assert_eq!(get_count(&addr), 0);
    }
}

