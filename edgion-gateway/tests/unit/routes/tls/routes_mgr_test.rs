use super::*;
use crate::routes::l4_rebuild::{rebuild_affected_l4_listener_managers, rebuild_all_l4_listener_managers};
use edgion_resources::resources::common::ParentReference;
use edgion_resources::resources::tls_route::TLSRouteSpec;

fn make_listener_key(port: u16, protocol: &str) -> ListenerKey {
    let addr = std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), port);
    ListenerKey::new(addr, protocol)
}

fn create_test_tls_route(namespace: &str, name: &str, listener_key: ListenerKey) -> TLSRoute {
    crate::test_current_status::processed(TLSRoute {
        metadata: kube::api::ObjectMeta {
            namespace: Some(namespace.to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: TLSRouteSpec {
            current_status: None,
            resolved_status_controller: Some("edgion.io/gateway-controller".into()),
            parent_refs: Some(vec![ParentReference {
                group: Some("gateway.networking.k8s.io".to_string()),
                kind: Some("Gateway".to_string()),
                namespace: Some(namespace.to_string()),
                name: "test-gateway".to_string(),
                section_name: Some("tls".to_string()),
                port: None,
            }]),
            hostnames: Some(vec!["test.example.com".into()]),
            rules: Some(vec![]),
            resolved_listeners: Some(vec![edgion_resources::resources::tls_route::ResolvedTlsListener {
                listener_key,
                gateway_namespace: namespace.into(),
                gateway_name: "test-gateway".into(),
                listener_name: "tls".into(),
                hostnames: vec!["test.example.com".into()],
            }]),
        },
        status: None,
    })
}

fn test_unit(namespace: &str, name: &str, listener_key: ListenerKey) -> Arc<TlsRouteUnit> {
    Arc::new(TlsRouteUnit::from_route(Arc::new(create_test_tls_route(
        namespace,
        name,
        listener_key,
    ))))
}

/// Drive the shared helper directly with a synthetic `active` PGIS
/// snapshot so tests do not race against the global store.
fn drive_rebuild_all(managers: &GlobalTlsRouteManagers, active: Vec<ListenerKey>) {
    rebuild_all_l4_listener_managers(
        "global_tls_route_managers",
        &managers.by_listener,
        &managers.route_cache,
        active,
        |u| resolved_listeners_for_route(&u.route),
        |k| managers.get_or_create_listener_manager(k),
        |m, units| m.rebuild(units),
    );
}

fn drive_rebuild_affected(
    managers: &GlobalTlsRouteManagers,
    affected: &HashSet<ListenerKey>,
    active: Vec<ListenerKey>,
) {
    rebuild_affected_l4_listener_managers(
        "global_tls_route_managers",
        &managers.by_listener,
        &managers.route_cache,
        affected,
        active,
        |u| resolved_listeners_for_route(&u.route),
        |k| managers.get_or_create_listener_manager(k),
        |m, units| m.rebuild(units),
    );
}

#[test]
fn stats_report_empty_and_retained_candidate_entries() {
    let managers = GlobalTlsRouteManagers::new();
    let empty = managers.stats();
    assert_eq!(empty.route_cache, 0);
    assert_eq!(empty.listener_count, 0);
    assert_eq!(empty.total_candidate_entries_in_tables, 0);

    let listener = make_listener_key(8442, "TLS");
    let manager = managers.get_or_create_listener_manager(&listener);
    let mut multi = create_test_tls_route("default", "multi", listener.clone());
    multi.spec.hostnames = Some(vec!["a.example.com".into(), "b.example.com".into()]);
    multi.spec.resolved_listeners.as_mut().unwrap()[0].hostnames = vec!["a.example.com".into(), "b.example.com".into()];
    let multi = Arc::new(TlsRouteUnit::from_route(Arc::new(multi)));
    let mut competitor = create_test_tls_route("default", "competitor", listener);
    competitor.spec.hostnames = Some(vec!["a.example.com".into()]);
    competitor.spec.resolved_listeners.as_mut().unwrap()[0].hostnames = vec!["a.example.com".into()];
    let competitor = Arc::new(TlsRouteUnit::from_route(Arc::new(competitor)));
    manager.rebuild(&HashMap::from([
        (multi.key_name(), multi),
        (competitor.key_name(), competitor),
    ]));

    let populated = managers.stats();
    assert_eq!(
        populated.route_cache, 0,
        "the listener snapshot is independently retained"
    );
    assert_eq!(populated.listener_count, 1);
    assert_eq!(populated.total_candidate_entries_in_tables, 3);
}

/// Regression for F-007: `rebuild_all` must clear the route table of an
/// active listener that previously had routes but now has none.
#[test]
fn test_rebuild_all_clears_active_listener_with_no_routes() {
    let managers = GlobalTlsRouteManagers::new();
    let listener_a = make_listener_key(8443, "TLS");
    let listener_b = make_listener_key(8444, "TLS");

    // Pre-seed B's manager with a prior route.
    let mgr_b = managers.get_or_create_listener_manager(&listener_b);
    let mut prior: HashMap<String, Arc<TlsRouteUnit>> = HashMap::new();
    prior.insert(
        "default/prior".to_string(),
        test_unit("default", "prior", listener_b.clone()),
    );
    mgr_b.rebuild(&prior);
    assert!(
        mgr_b.load_route_table().match_route("test.example.com").is_some(),
        "precondition: B has a non-empty table"
    );

    // route_cache only references A.
    managers.insert_route(test_unit("default", "route-a", listener_a.clone()));

    drive_rebuild_all(&managers, vec![listener_a.clone(), listener_b.clone()]);

    assert!(
        mgr_b.load_route_table().match_route("test.example.com").is_none(),
        "active-but-no-routes TLS listener should be cleared"
    );
    assert!(managers.by_listener.contains_key(&listener_a));
    assert!(managers.by_listener.contains_key(&listener_b));
}

#[test]
fn test_rebuild_all_removes_stale_inactive_listener() {
    let managers = GlobalTlsRouteManagers::new();
    let stale = make_listener_key(8445, "TLS");
    managers.get_or_create_listener_manager(&stale);
    assert!(managers.by_listener.contains_key(&stale));

    drive_rebuild_all(&managers, vec![]);

    assert!(
        !managers.by_listener.contains_key(&stale),
        "inactive-and-no-routes TLS by_listener entry should be removed"
    );
}

#[test]
fn test_rebuild_affected_clears_listener_with_empty_bucket() {
    let managers = GlobalTlsRouteManagers::new();
    let listener = make_listener_key(8446, "TLS");

    let mgr = managers.get_or_create_listener_manager(&listener);
    let mut prior: HashMap<String, Arc<TlsRouteUnit>> = HashMap::new();
    prior.insert(
        "default/prior".to_string(),
        test_unit("default", "prior", listener.clone()),
    );
    mgr.rebuild(&prior);
    assert!(
        mgr.load_route_table().match_route("test.example.com").is_some(),
        "precondition: listener has the prior route"
    );
    let populated = managers.stats();
    assert_eq!(populated.listener_count, 1);
    assert_eq!(populated.total_candidate_entries_in_tables, 1);

    let mut affected: HashSet<ListenerKey> = HashSet::new();
    affected.insert(listener.clone());
    drive_rebuild_affected(&managers, &affected, vec![listener.clone()]);

    assert!(
        mgr.load_route_table().match_route("test.example.com").is_none(),
        "affected TLS listener with no routes should be cleared"
    );
    assert!(managers.by_listener.contains_key(&listener));
    let cleared = managers.stats();
    assert_eq!(cleared.listener_count, 1);
    assert_eq!(cleared.total_candidate_entries_in_tables, 0);
}

#[test]
fn test_rebuild_affected_orphan_sweep() {
    let managers = GlobalTlsRouteManagers::new();
    let affected_key = make_listener_key(8447, "TLS");
    let orphan_key = make_listener_key(8448, "TLS");

    managers.get_or_create_listener_manager(&orphan_key);
    managers.insert_route(test_unit("default", "route-x", affected_key.clone()));

    let mut affected: HashSet<ListenerKey> = HashSet::new();
    affected.insert(affected_key.clone());
    drive_rebuild_affected(&managers, &affected, vec![affected_key.clone()]);

    assert!(!managers.by_listener.contains_key(&orphan_key));
    assert!(managers.by_listener.contains_key(&affected_key));
}
