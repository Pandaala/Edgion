use super::*;
use edgion_resources::resources::common::ParentReference;
use edgion_resources::resources::tcp_route::*;
use edgion_resources::ResourceMeta;

fn create_test_tcp_route(namespace: &str, name: &str, port: u16) -> TCPRoute {
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
                port: Some(port as i32),
            }]),
            rules: Some(vec![]),
            resolved_listeners: None,
            resolved_listener_attachments: None,
        },
        status: None,
    })
}

#[test]
fn test_match_route_is_deterministic() {
    // Two routes on the same listener (invariant violation). Insert in the
    // order "b/route" then "a/route"; match_route must return "a/route"
    // regardless of HashMap iteration order, proving the build-time sort.
    let route_b = Arc::new(create_test_tcp_route("b", "route", 9000));
    let route_a = Arc::new(create_test_tcp_route("a", "route", 9000));

    let mut units = HashMap::new();
    units.insert(route_b.key_name(), Arc::new(TcpRouteUnit::from_route(route_b).unwrap()));
    units.insert(route_a.key_name(), Arc::new(TcpRouteUnit::from_route(route_a).unwrap()));

    let table = TcpRouteTable::from_units(&units);
    assert_eq!(table.route_count(), 2);
    assert_eq!(table.match_route().unwrap().key_name(), "a/route");
}

#[test]
fn test_empty_table() {
    let table = TcpRouteTable::new();
    assert!(table.match_route().is_none());
    assert_eq!(table.route_count(), 0);
}
