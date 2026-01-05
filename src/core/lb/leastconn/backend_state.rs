//! Backend lifecycle state management for LeastConnection load balancing
//!
//! Manages backend states during graceful shutdown and reactivation scenarios.

use dashmap::DashMap;
use pingora_core::protocols::l4::socket::SocketAddr;
use std::sync::LazyLock;

/// Backend lifecycle states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendState {
    /// Active: accepting new connections
    Active,
    /// Draining: no new connections, waiting for existing to close
    Draining,
    /// Removed: ready for cleanup (count = 0)
    Removed,
}

/// Global backend state tracker
static BACKEND_STATES: LazyLock<DashMap<SocketAddr, BackendState>> = LazyLock::new(DashMap::new);

/// Set backend state
pub fn set_state(addr: &SocketAddr, state: BackendState) {
    BACKEND_STATES.insert(addr.clone(), state);
}

/// Get backend state (default: Active for new backends)
pub fn get_state(addr: &SocketAddr) -> BackendState {
    BACKEND_STATES.get(addr).map(|s| *s).unwrap_or(BackendState::Active)
}

/// Check if backend is active (can accept new connections)
pub fn is_active(addr: &SocketAddr) -> bool {
    matches!(get_state(addr), BackendState::Active)
}

/// Mark backend as draining (has connections but removed from pool)
pub fn mark_draining(addr: &SocketAddr) {
    set_state(addr, BackendState::Draining);
    tracing::info!(
        backend = %addr,
        "Backend marked as draining, will not accept new connections"
    );
}

/// Mark backend as removed (ready for cleanup)
pub fn mark_removed(addr: &SocketAddr) {
    set_state(addr, BackendState::Removed);
}

/// Reactivate a draining backend (when re-added to pool)
pub fn reactivate(addr: &SocketAddr) {
    set_state(addr, BackendState::Active);
    tracing::info!(
        backend = %addr,
        "Backend reactivated, accepting new connections"
    );
}

/// Remove backend state entry
pub fn remove(addr: &SocketAddr) {
    BACKEND_STATES.remove(addr);
}

/// Get all draining backends
pub fn get_draining_backends() -> Vec<SocketAddr> {
    BACKEND_STATES
        .iter()
        .filter(|entry| *entry.value() == BackendState::Draining)
        .map(|entry| entry.key().clone())
        .collect()
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
    fn test_default_state_is_active() {
        let addr = make_addr(9991);
        assert_eq!(get_state(&addr), BackendState::Active);
        assert!(is_active(&addr));
    }

    #[test]
    fn test_mark_draining() {
        let addr = make_addr(9992);
        mark_draining(&addr);
        assert_eq!(get_state(&addr), BackendState::Draining);
        assert!(!is_active(&addr));
    }

    #[test]
    fn test_reactivate() {
        let addr = make_addr(9993);
        mark_draining(&addr);
        assert!(!is_active(&addr));

        reactivate(&addr);
        assert_eq!(get_state(&addr), BackendState::Active);
        assert!(is_active(&addr));
    }

    #[test]
    fn test_remove() {
        let addr = make_addr(9994);
        set_state(&addr, BackendState::Draining);
        assert_eq!(get_state(&addr), BackendState::Draining);

        remove(&addr);
        // After removal, should return default (Active)
        assert_eq!(get_state(&addr), BackendState::Active);
    }

    #[test]
    fn test_get_draining_backends() {
        let addr1 = make_addr(19995);
        let addr2 = make_addr(19996);
        let addr3 = make_addr(19997);

        // Clean up any previous state
        remove(&addr1);
        remove(&addr2);
        remove(&addr3);

        mark_draining(&addr1);
        set_state(&addr2, BackendState::Active);
        mark_draining(&addr3);

        let draining = get_draining_backends();
        // Filter to only our test addresses
        let test_draining: Vec<_> = draining
            .iter()
            .filter(|addr| matches!(addr, SocketAddr::Inet(sa) if sa.port() >= 19995 && sa.port() <= 19997))
            .collect();

        assert_eq!(test_draining.len(), 2);
        assert!(draining.contains(&addr1));
        assert!(draining.contains(&addr3));
        assert!(!draining.contains(&addr2));

        // Cleanup
        remove(&addr1);
        remove(&addr2);
        remove(&addr3);
    }
}
