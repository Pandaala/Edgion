use crate::types::resources::TCPRoute;
use std::collections::HashMap;
use std::sync::Arc;

/// Per-port TCP route table — immutable snapshot shared via ArcSwap.
///
/// TCP listeners have simpler semantics than TLS: there is no hostname-based
/// matching (no SNI equivalent). Per Gateway API, each TCP listener binds at
/// most one TCPRoute, so the table stores a flat list with first-match
/// semantics.
///
/// A new snapshot is built and atomically swapped on every route change,
/// ensuring all readers always see consistent data without locking.
pub struct TcpRouteTable {
    routes: Vec<Arc<TCPRoute>>,
}

impl TcpRouteTable {
    pub fn new() -> Self {
        Self { routes: Vec::new() }
    }

    /// Build a TcpRouteTable from a flat set of routes belonging to one port.
    pub fn from_routes(routes: &HashMap<String, Arc<TCPRoute>>) -> Self {
        let routes: Vec<Arc<TCPRoute>> = routes.values().cloned().collect();
        Self { routes }
    }

    /// Match a TCPRoute (first-match semantics).
    ///
    /// TCP has no hostname dimension, so we simply return the first available
    /// route for this port.
    pub fn match_route(&self) -> Option<Arc<TCPRoute>> {
        self.routes.first().cloned()
    }

    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}

impl Default for TcpRouteTable {
    fn default() -> Self {
        Self::new()
    }
}