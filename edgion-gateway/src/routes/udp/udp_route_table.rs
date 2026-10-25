use crate::routes::udp::route_unit::UdpRouteUnit;
use std::collections::HashMap;
use std::sync::Arc;

/// Per-port UDP route table — immutable snapshot shared via ArcSwap.
///
/// UDP listeners have simpler semantics than TLS: there is no hostname-based
/// matching (no SNI equivalent). Per Gateway API, each UDP listener binds at
/// most one UDPRoute, so the table stores a flat list with first-match
/// semantics.
///
/// Each entry is a gateway-side [`UdpRouteUnit`] compiled once by the
/// ConfHandler and cached by `GlobalUdpRouteManagers`; table rebuilds only
/// clone unit `Arc`s into a listener snapshot.
///
/// Full updates publish all listener snapshots, while partial updates publish
/// only affected listeners. Readers always see a consistent snapshot without
/// locking.
pub struct UdpRouteTable {
    routes: Vec<Arc<UdpRouteUnit>>,
}

impl UdpRouteTable {
    pub fn new() -> Self {
        Self { routes: Vec::new() }
    }

    /// Build a UdpRouteTable from already-compiled units belonging to one port.
    ///
    /// Units are built ONCE in the conf-handler and cached, so this only
    /// Arc-clones them into the snapshot — the per-route `backend_finder` (and
    /// its WRR cursor) is preserved across unrelated route-table rebuilds.
    ///
    /// Place the minimum resource key (`namespace/name`) first so `match_route`'s first-match
    /// pick is deterministic across `HashMap` iteration order, process restarts,
    /// and pod replicas. Per Gateway API a UDP listener binds at most one route;
    /// the selection only matters as a stable fallback when that invariant is violated
    /// (operator misconfig or a controller event race), mirroring the
    /// `AlphabeticalRoute` tiebreak.
    pub fn from_units(units: &HashMap<String, Arc<UdpRouteUnit>>) -> Self {
        let mut units: Vec<Arc<UdpRouteUnit>> = units.values().cloned().collect();
        // Only the first entry is matched; retain the rest for snapshot ownership
        // and route counts without sorting them. Equal keys keep the first input.
        if units.len() > 1 {
            if let Some(index) = units
                .iter()
                .enumerate()
                .min_by_key(|(_, u)| u.key_name())
                .map(|(i, _)| i)
            {
                units.swap(0, index);
            }
        }
        Self { routes: units }
    }

    /// Match a UDPRoute (first-match semantics).
    ///
    /// UDP has no hostname dimension, so we simply return the first available
    /// route for this port.
    pub fn match_route(&self) -> Option<Arc<UdpRouteUnit>> {
        self.routes.first().cloned()
    }

    pub fn route_count(&self) -> usize {
        self.routes.len()
    }
}

impl Default for UdpRouteTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/routes/udp/udp_route_table_test.rs"]
mod tests;
