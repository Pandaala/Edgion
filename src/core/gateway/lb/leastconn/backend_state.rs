//! Backend lifecycle state enum used by service-scoped runtime state.

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
