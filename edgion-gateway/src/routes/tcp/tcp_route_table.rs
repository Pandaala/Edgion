use crate::routes::tcp::route_unit::TcpRouteUnit;
use std::collections::HashMap;
use std::sync::Arc;

/// Per-port TCP route table — immutable snapshot shared via ArcSwap.
///
/// TCP listeners have simpler semantics than TLS: there is no hostname-based
/// matching (no SNI equivalent). Per Gateway API, each TCP listener binds at
/// most one TCPRoute, so the table stores a flat list with first-match
/// semantics.
///
/// Each entry is a gateway-side [`TcpRouteUnit`] (compiled companion) built
/// from the initialised `Arc<TCPRoute>` once at rebuild time.
///
/// A new snapshot is built and atomically swapped on every route change,
/// ensuring all readers always see consistent data without locking.
pub struct TcpRouteTable {
    routes: Vec<Arc<TcpRouteUnit>>,
}

impl TcpRouteTable {
    pub fn new() -> Self {
        Self { routes: Vec::new() }
    }

    /// Build a TcpRouteTable from already-compiled units belonging to one port.
    ///
    /// Units are built ONCE in the conf-handler and cached, so this only
    /// Arc-clones them into the snapshot — the per-route `backend_finder` (and
    /// its WRR cursor) is preserved across unrelated route-table rebuilds.
    ///
    /// Sort by resource key (`namespace/name`) so `match_route`'s first-match
    /// pick is deterministic across `HashMap` iteration order, process restarts,
    /// and pod replicas. Per Gateway API a TCP listener binds at most one route;
    /// the sort only matters as a stable fallback when that invariant is violated
    /// (operator misconfig or a controller event race), mirroring the
    /// `AlphabeticalRoute` tiebreak.
    pub fn from_units(units: &HashMap<String, Arc<TcpRouteUnit>>) -> Self {
        let mut units: Vec<Arc<TcpRouteUnit>> = units.values().cloned().collect();
        units.sort_by_key(|u| u.key_name());
        Self { routes: units }
    }

    /// Match a TCPRoute (first-match semantics).
    ///
    /// TCP has no hostname dimension, so we simply return the first available
    /// route for this port.
    pub fn match_route(&self) -> Option<Arc<TcpRouteUnit>> {
        self.routes.first().cloned()
    }

    pub fn route_count(&self) -> usize {
        self.routes.len()
    }
}

impl Default for TcpRouteTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/routes/tcp/tcp_route_table_test.rs"]
mod tests;
