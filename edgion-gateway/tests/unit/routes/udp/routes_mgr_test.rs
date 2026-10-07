use super::*;
use edgion_resources::resources::udp_route::*;

fn make_listener_key(port: u16, protocol: &str) -> ListenerKey {
    let addr = std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), port);
    ListenerKey::new(addr, protocol)
}

fn create_test_udp_route(namespace: &str, name: &str, listener_key: ListenerKey) -> UDPRoute {
    use edgion_resources::resources::common::ParentReference;
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
                port: None,
            }]),
            rules: Some(vec![]),
            resolved_listeners: Some(vec![listener_key]),
            resolved_listener_attachments: None,
        },
        status: None,
    })
}

fn test_unit(namespace: &str, name: &str, listener_key: ListenerKey) -> Arc<UdpRouteUnit> {
    Arc::new(UdpRouteUnit::from_route(Arc::new(create_test_udp_route(namespace, name, listener_key))).unwrap())
}

#[test]
fn test_get_or_create_listener_manager() {
    let managers = GlobalUdpRouteManagers::new();
    let key1 = make_listener_key(9000, "UDP");
    let mgr1 = managers.get_or_create_listener_manager(&key1);
    let mgr2 = managers.get_or_create_listener_manager(&key1);
    assert!(Arc::ptr_eq(&mgr1, &mgr2));

    let key2 = make_listener_key(9001, "UDP");
    let mgr3 = managers.get_or_create_listener_manager(&key2);
    assert!(!Arc::ptr_eq(&mgr1, &mgr3));
}

#[test]
fn test_insert_and_get_route() {
    let managers = GlobalUdpRouteManagers::new();
    let key = make_listener_key(9000, "UDP");
    let unit = test_unit("default", "route1", key);
    let route_key = unit.key_name();

    managers.insert_route(unit.clone());
    let found = managers.get_route(&route_key);
    assert!(found.is_some());

    managers.remove_route(&route_key);
    let found = managers.get_route(&route_key);
    assert!(found.is_none());
}

#[test]
fn stats_report_empty_and_retained_table_routes() {
    let managers = GlobalUdpRouteManagers::new();
    let empty = managers.stats();
    assert_eq!(empty.route_cache, 0);
    assert_eq!(empty.listener_count, 0);
    assert_eq!(empty.total_routes_in_tables, 0);

    let listener_a = make_listener_key(9000, "UDP");
    let listener_b = make_listener_key(9001, "UDP");
    let unit = test_unit("default", "route1", listener_a.clone());
    let routes = HashMap::from([(unit.key_name(), unit)]);
    managers.get_or_create_listener_manager(&listener_a).rebuild(&routes);
    managers.get_or_create_listener_manager(&listener_b).rebuild(&routes);

    let stats = managers.stats();
    assert_eq!(stats.route_cache, 0, "listener tables retain their own snapshots");
    assert_eq!(stats.listener_count, 2);
    assert_eq!(
        stats.total_routes_in_tables, 2,
        "entries are counted per listener table"
    );
}

#[test]
fn test_clear_route_cache() {
    let managers = GlobalUdpRouteManagers::new();
    let key = make_listener_key(9000, "UDP");
    managers.insert_route(test_unit("default", "route1", key));

    assert_eq!(managers.stats().route_cache, 1);
    managers.clear_route_cache();
    assert_eq!(managers.stats().route_cache, 0);
}

/// Drive the shared helper directly with a synthetic `active` PGIS
/// snapshot so tests do not race against the global store.
fn drive_rebuild_all(managers: &GlobalUdpRouteManagers, active: Vec<ListenerKey>) {
    crate::routes::l4_rebuild::rebuild_all_l4_listener_managers(
        "global_udp_route_managers",
        &managers.by_listener,
        &managers.route_cache,
        active,
        |u| resolved_listeners_for_route(u.route()).to_vec(),
        |k| managers.get_or_create_listener_manager(k),
        |m, units| m.rebuild(units),
    );
}

fn drive_rebuild_affected(
    managers: &GlobalUdpRouteManagers,
    affected: &HashSet<ListenerKey>,
    active: Vec<ListenerKey>,
) {
    crate::routes::l4_rebuild::rebuild_affected_l4_listener_managers(
        "global_udp_route_managers",
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
/// that previously had routes but now has none.
#[test]
fn test_rebuild_all_clears_active_listener_with_no_routes() {
    let managers = GlobalUdpRouteManagers::new();
    let listener_a = make_listener_key(9500, "UDP");
    let listener_b = make_listener_key(9501, "UDP");

    let mgr_b = managers.get_or_create_listener_manager(&listener_b);
    let mut prior: HashMap<String, Arc<UdpRouteUnit>> = HashMap::new();
    prior.insert(
        "default/prior".to_string(),
        test_unit("default", "prior", listener_b.clone()),
    );
    mgr_b.rebuild(&prior);
    assert_eq!(mgr_b.load_route_table().route_count(), 1, "precondition: B has 1 route");

    managers.insert_route(test_unit("default", "route-a", listener_a.clone()));

    drive_rebuild_all(&managers, vec![listener_a.clone(), listener_b.clone()]);

    assert_eq!(
        mgr_b.load_route_table().route_count(),
        0,
        "active-but-no-routes UDP listener should be cleared"
    );
    let mgr_a = managers
        .by_listener
        .get(&listener_a)
        .map(|e| e.value().clone())
        .unwrap();
    assert_eq!(mgr_a.load_route_table().route_count(), 1);
    assert!(managers.by_listener.contains_key(&listener_a));
    assert!(managers.by_listener.contains_key(&listener_b));
}

#[test]
fn test_rebuild_affected_clears_listener_with_empty_bucket() {
    let managers = GlobalUdpRouteManagers::new();
    let listener = make_listener_key(9600, "UDP");

    let mgr = managers.get_or_create_listener_manager(&listener);
    let mut prior: HashMap<String, Arc<UdpRouteUnit>> = HashMap::new();
    prior.insert(
        "default/prior".to_string(),
        test_unit("default", "prior", listener.clone()),
    );
    mgr.rebuild(&prior);
    assert_eq!(mgr.load_route_table().route_count(), 1);

    let mut affected: HashSet<ListenerKey> = HashSet::new();
    affected.insert(listener.clone());
    drive_rebuild_affected(&managers, &affected, vec![listener.clone()]);

    assert_eq!(mgr.load_route_table().route_count(), 0);
    assert!(managers.by_listener.contains_key(&listener));
    let cleared = managers.stats();
    assert_eq!(cleared.route_cache, 0);
    assert_eq!(cleared.listener_count, 1);
    assert_eq!(cleared.total_routes_in_tables, 0);
}
