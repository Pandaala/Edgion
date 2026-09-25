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
            resolved_listener_attachments: None,
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
