use super::*;
use crate::routes::l4_rebuild::{rebuild_affected_l4_listener_managers, rebuild_all_l4_listener_managers};
use edgion_resources::resources::common::ParentReference;
use edgion_resources::resources::tcp_route::TCPRouteSpec;

fn make_listener_key(port: u16, protocol: &str) -> ListenerKey {
    let addr = std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), port);
    ListenerKey::new(addr, protocol)
}

fn create_test_tcp_route(namespace: &str, name: &str, listener_key: ListenerKey) -> TCPRoute {
    crate::test_current_status::processed(TCPRoute {
        metadata: kube::api::ObjectMeta {
            namespace: Some(namespace.to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: TCPRouteSpec {
            current_status: None,
            resolved_status_controller: Some("edgion.io/gateway-controller".into()),
            parent_refs: Some(vec![ParentReference {
                group: Some("gateway.networking.k8s.io".to_string()),
                kind: Some("Gateway".to_string()),
                namespace: Some(namespace.to_string()),
                name: "test-gateway".to_string(),
                section_name: Some("tcp".to_string()),
                port: None,
            }]),
            rules: Some(vec![]),
            resolved_listeners: Some(vec![listener_key]),
            resolved_listener_attachments: None,
        },
        status: None,
    })
}

fn test_unit(namespace: &str, name: &str, listener_key: ListenerKey) -> Arc<TcpRouteUnit> {
    Arc::new(TcpRouteUnit::from_route(Arc::new(create_test_tcp_route(namespace, name, listener_key))).unwrap())
}

/// Drive the shared helper directly with a synthetic `active` PGIS snapshot
/// so tests do not race against the global `ListenerGatewayInfoStore`.
fn drive_rebuild_all(managers: &GlobalTcpRouteManagers, active: Vec<ListenerKey>) {
    rebuild_all_l4_listener_managers(
        "global_tcp_route_managers",
        &managers.by_listener,
        &managers.route_cache,
        active,
        |u| resolved_listeners_for_route(u.route()).to_vec(),
        |k| managers.get_or_create_listener_manager(k),
        |m, units| m.rebuild(units),
    );
}

fn drive_rebuild_affected(
    managers: &GlobalTcpRouteManagers,
    affected: &HashSet<ListenerKey>,
    active: Vec<ListenerKey>,
) {
    rebuild_affected_l4_listener_managers(
        "global_tcp_route_managers",
        &managers.by_listener,
        &managers.route_cache,
        affected,
        active,
        |u| resolved_listeners_for_route(u.route()).to_vec(),
        |k| managers.get_or_create_listener_manager(k),
        |m, units| m.rebuild(units),
    );
}

/// Invariant: `rebuild_all` must clear the route table of an active listener
/// that previously had routes but now has none. Without the clear-pass, the
/// listener's `ArcSwap<TcpRouteTable>` retains the previous snapshot and the
/// data plane keeps forwarding to deleted backends.
#[test]
fn test_rebuild_all_clears_active_listener_with_no_routes() {
    let managers = GlobalTcpRouteManagers::new();
    let listener_a = make_listener_key(9000, "TCP");
    let listener_b = make_listener_key(9001, "TCP");

    // Seed B's manager with a prior route to mimic state from an earlier
    // rebuild_all (the by_listener entry pre-exists).
    let mgr_b = managers.get_or_create_listener_manager(&listener_b);
    let mut prior: HashMap<String, Arc<TcpRouteUnit>> = HashMap::new();
    prior.insert(
        "default/prior".to_string(),
        test_unit("default", "prior", listener_b.clone()),
    );
    mgr_b.rebuild(&prior);
    assert_eq!(mgr_b.load_route_table().route_count(), 1, "precondition: B has 1 route");

    // Current route_cache only references listener A.
    managers.insert_route(test_unit("default", "route-a", listener_a.clone()));

    // Both A and B are still active in PGIS.
    drive_rebuild_all(&managers, vec![listener_a.clone(), listener_b.clone()]);

    // B's snapshot must be cleared, not stale.
    assert_eq!(
        mgr_b.load_route_table().route_count(),
        0,
        "active-but-no-routes listener should be cleared"
    );
    // A should hold its current route.
    let mgr_a = managers
        .by_listener
        .get(&listener_a)
        .map(|e| e.value().clone())
        .unwrap();
    assert_eq!(mgr_a.load_route_table().route_count(), 1, "A retains its route");
    // Neither is removed from by_listener (both still active).
    assert!(managers.by_listener.contains_key(&listener_a));
    assert!(managers.by_listener.contains_key(&listener_b));
}

/// `rebuild_all` drops `by_listener` entries that have no routes AND are
/// no longer active in PGIS.
#[test]
fn test_rebuild_all_removes_stale_inactive_listener() {
    let managers = GlobalTcpRouteManagers::new();
    let stale = make_listener_key(9100, "TCP");

    // Retain the same manager Arc a live proxy would hold and seed a real route.
    let retained = managers.get_or_create_listener_manager(&stale);
    let mut prior = HashMap::new();
    prior.insert(
        "default/prior".to_string(),
        test_unit("default", "prior", stale.clone()),
    );
    retained.rebuild(&prior);
    assert_eq!(retained.load_route_table().route_count(), 1);
    assert!(managers.by_listener.contains_key(&stale));

    // No routes, no active listeners.
    drive_rebuild_all(&managers, vec![]);

    assert!(
        !managers.by_listener.contains_key(&stale),
        "by_listener entry with no routes and not in PGIS should be removed"
    );
    assert_eq!(
        retained.load_route_table().route_count(),
        0,
        "a retained handle must be cleared when its registry entry is removed"
    );
    let recreated = managers.get_or_create_listener_manager(&stale);
    assert!(!Arc::ptr_eq(&retained, &recreated));
}

/// `rebuild_affected` rebuilds only the listeners passed in `affected`,
/// and on an affected listener that has no routes, clears its snapshot.
#[test]
fn test_rebuild_affected_clears_listener_with_empty_bucket() {
    let managers = GlobalTcpRouteManagers::new();
    let listener = make_listener_key(9200, "TCP");

    // Pre-seed the listener with a route so the route_table is non-empty.
    let mgr = managers.get_or_create_listener_manager(&listener);
    let mut prior: HashMap<String, Arc<TcpRouteUnit>> = HashMap::new();
    prior.insert(
        "default/prior".to_string(),
        test_unit("default", "prior", listener.clone()),
    );
    mgr.rebuild(&prior);
    assert_eq!(mgr.load_route_table().route_count(), 1);

    // route_cache is empty (all routes for this listener were removed); the
    // listener is still active in PGIS and is in `affected`.
    let mut affected: HashSet<ListenerKey> = HashSet::new();
    affected.insert(listener.clone());
    drive_rebuild_affected(&managers, &affected, vec![listener.clone()]);

    assert_eq!(
        mgr.load_route_table().route_count(),
        0,
        "affected listener with no routes should be cleared"
    );
    assert!(
        managers.by_listener.contains_key(&listener),
        "active listener stays in by_listener even when cleared"
    );
}

/// `rebuild_affected` orphan-sweep removes by_listener entries that are
/// not referenced by any current route and no longer active in PGIS.
#[test]
fn test_rebuild_affected_orphan_sweep() {
    let managers = GlobalTcpRouteManagers::new();
    let affected_key = make_listener_key(9300, "TCP");
    let orphan_key = make_listener_key(9301, "TCP");

    // Retain and seed the orphan manager as a live proxy could have done
    // before its listener disappeared from PGIS.
    let retained_orphan = managers.get_or_create_listener_manager(&orphan_key);
    let mut prior = HashMap::new();
    prior.insert(
        "default/orphan-prior".to_string(),
        test_unit("default", "orphan-prior", orphan_key.clone()),
    );
    retained_orphan.rebuild(&prior);
    assert_eq!(retained_orphan.load_route_table().route_count(), 1);
    // And the affected listener which currently has a route.
    managers.insert_route(test_unit("default", "route-x", affected_key.clone()));

    let mut affected: HashSet<ListenerKey> = HashSet::new();
    affected.insert(affected_key.clone());
    drive_rebuild_affected(&managers, &affected, vec![affected_key.clone()]);

    assert!(
        !managers.by_listener.contains_key(&orphan_key),
        "orphan by_listener entry should be swept"
    );
    assert_eq!(
        retained_orphan.load_route_table().route_count(),
        0,
        "the orphan sweep must clear a retained manager handle"
    );
    let recreated = managers.get_or_create_listener_manager(&orphan_key);
    assert!(!Arc::ptr_eq(&retained_orphan, &recreated));
    assert!(managers.by_listener.contains_key(&affected_key));
}

/// Invariant: a listener-table rebuild must Arc-CLONE the cached unit, never
/// rebuild it. If the served unit is the same `Arc` as the cached one, its
/// `backend_finder` (and WRR cursor) is preserved across rebuilds, so an
/// unrelated route/listener change does not reset a route's load-balancing
/// distribution. A reintroduction of "build the unit inside `from_units`"
/// would make `ptr_eq` fail here.
#[test]
fn test_unit_arc_reused_across_table_rebuild() {
    let managers = GlobalTcpRouteManagers::new();
    let key = make_listener_key(9700, "TCP");
    managers.insert_route(test_unit("default", "route1", key.clone()));
    let cached = managers.get_route("default/route1").unwrap();

    // Gateway-driven rebuild of the listener's table.
    drive_rebuild_all(&managers, vec![key.clone()]);
    let served = managers
        .by_listener
        .get(&key)
        .unwrap()
        .load_route_table()
        .match_route()
        .unwrap();
    assert!(
        Arc::ptr_eq(&cached, &served),
        "table must serve the cached unit Arc, not a freshly-built one"
    );

    // A second rebuild must still serve the very same Arc.
    drive_rebuild_all(&managers, vec![key.clone()]);
    let served2 = managers
        .by_listener
        .get(&key)
        .unwrap()
        .load_route_table()
        .match_route()
        .unwrap();
    assert!(
        Arc::ptr_eq(&cached, &served2),
        "unit Arc must survive repeated rebuilds"
    );
}
