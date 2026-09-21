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
    /// Sort by resource key (`namespace/name`) so `match_route`'s first-match
    /// pick is deterministic across `HashMap` iteration order, process restarts,
    /// and pod replicas. Per Gateway API a UDP listener binds at most one route;
    /// the sort only matters as a stable fallback when that invariant is violated
    /// (operator misconfig or a controller event race), mirroring the
    /// `AlphabeticalRoute` tiebreak.
    pub fn from_units(units: &HashMap<String, Arc<UdpRouteUnit>>) -> Self {
        let mut units: Vec<Arc<UdpRouteUnit>> = units.values().cloned().collect();
        units.sort_by_key(|u| u.key_name());
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
mod tests {
    use super::*;
    use edgion_resources::resources::common::ParentReference;
    use edgion_resources::resources::udp_route::*;

    fn create_test_udp_route(namespace: &str, name: &str, port: u16) -> UDPRoute {
        crate::test_current_status::processed(UDPRoute {
            metadata: kube::api::ObjectMeta {
                namespace: Some(namespace.to_string()),
                name: Some(name.to_string()),
                ..Default::default()
            },
            spec: UDPRouteSpec {
                current_status: None,
                resolved_status_controller: Some("edgion.io/gateway-controller".into()),
                parent_refs: Some(vec![ParentReference {
                    group: Some("gateway.networking.k8s.io".to_string()),
                    kind: Some("Gateway".to_string()),
                    namespace: Some(namespace.to_string()),
                    name: "test-gateway".to_string(),
                    section_name: Some("udp".to_string()),
                    port: Some(port as i32),
                }]),
                rules: Some(vec![]),
                resolved_listeners: None,
            },
            status: None,
        })
    }

    fn test_unit(namespace: &str, name: &str, port: u16) -> Arc<UdpRouteUnit> {
        Arc::new(UdpRouteUnit::from_route(Arc::new(create_test_udp_route(
            namespace, name, port,
        ))))
    }

    #[test]
    fn test_match_route_returns_first() {
        let unit = test_unit("default", "route1", 9000);
        let key = unit.key_name();
        let mut units = HashMap::new();
        units.insert(key.clone(), unit);

        let table = UdpRouteTable::from_units(&units);
        let matched = table.match_route();
        assert!(matched.is_some());
        assert_eq!(matched.unwrap().key_name(), key);
    }

    #[test]
    fn test_empty_table() {
        let table = UdpRouteTable::new();
        assert!(table.match_route().is_none());
        assert_eq!(table.route_count(), 0);
    }

    #[test]
    fn test_match_route_is_deterministic() {
        // Two routes on the same listener (invariant violation). Insert in the
        // order "b/route" then "a/route"; match_route must return "a/route"
        // regardless of HashMap iteration order, proving the build-time sort.
        let unit_b = test_unit("b", "route", 9000);
        let unit_a = test_unit("a", "route", 9000);

        let mut units = HashMap::new();
        units.insert(unit_b.key_name(), unit_b);
        units.insert(unit_a.key_name(), unit_a);

        let table = UdpRouteTable::from_units(&units);
        assert_eq!(table.route_count(), 2);
        assert_eq!(table.match_route().unwrap().key_name(), "a/route");
    }

    #[test]
    fn test_from_routes() {
        let unit1 = test_unit("default", "route1", 9000);
        let unit2 = test_unit("default", "route2", 9000);

        let mut units = HashMap::new();
        units.insert(unit1.key_name(), unit1);
        units.insert(unit2.key_name(), unit2);

        let table = UdpRouteTable::from_units(&units);
        assert_eq!(table.route_count(), 2);
        assert!(table.match_route().is_some());
    }
}
