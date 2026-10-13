use super::*;

#[test]
fn owner_and_status_updates_replace_acceptance_with_unchanged_listeners() {
    let managers = GlobalTcpRouteManagers::new();
    let listener = make_listener_key(19443, "TCP");
    let mut route = create_test_tcp_route("default", "owner-update", "gateway", "tcp", 19443, listener.clone());
    let key = "default/owner-update".to_string();
    let parent = route.spec.parent_refs.as_ref().unwrap()[0].clone();
    route.status = Some(
        serde_json::from_value(serde_json::json!({"parents": [{
            "parentRef": parent, "controllerName": "example.net/owner",
            "conditions": [{"type": "Accepted", "status": "False", "reason": "Rejected",
                "message": "", "lastTransitionTime": "2026-09-20T00:00:00Z"}]
        }]}))
        .unwrap(),
    );
    route.spec.resolved_status_controller = Some("example.net/other".into());
    route.spec.current_status.as_mut().unwrap().parents[0].controller_name = "example.net/other".into();
    managers.full_set(&HashMap::from([(key.clone(), Arc::new(route.clone()))]));
    let original = managers.get_route(&key).unwrap();
    assert_eq!(original.resolved_parent_refs().len(), 1);

    // The current owner rejects while the complete status reports acceptance.
    route.spec.resolved_status_controller = Some("example.net/owner".into());
    route.spec.current_status = route.status.clone();
    route.status.as_mut().unwrap().parents[0].conditions[0].status = "True".into();
    managers.partial_update(
        HashMap::new(),
        HashMap::from([(key.clone(), Arc::new(route.clone()))]),
        HashSet::new(),
    );
    assert!(managers.get_route(&key).unwrap().resolved_parent_refs().is_empty());
    assert_eq!(
        original.resolved_parent_refs().len(),
        1,
        "in-flight snapshot stays immutable"
    );

    // Only status changes, with exactly the same listener attachment.
    route.spec.current_status.as_mut().unwrap().parents[0].controller_name = "example.net/owner".into();
    route.spec.current_status.as_mut().unwrap().parents[0].conditions[0].status = "True".into();
    route.status.as_mut().unwrap().parents[0].conditions[0].status = "False".into();
    managers.partial_update(
        HashMap::new(),
        HashMap::from([(key.clone(), Arc::new(route.clone()))]),
        HashSet::new(),
    );
    assert_eq!(managers.get_route(&key).unwrap().resolved_parent_refs().len(), 1);
    route.spec.resolved_status_controller = None;
    managers.full_set(&HashMap::from([(key.clone(), Arc::new(route))]));
    assert!(managers.get_route(&key).unwrap().resolved_parent_refs().is_empty());
    managers.partial_update(HashMap::new(), HashMap::new(), HashSet::from([key.clone()]));
    assert!(managers.get_route(&key).is_none());
}
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::resources::common::ParentReference;
use edgion_resources::resources::tcp_route::*;
use edgion_resources::ListenerKey;

fn make_listener_key(port: u16, protocol: &str) -> ListenerKey {
    let addr = std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), port);
    ListenerKey::new(addr, protocol)
}

fn create_test_tcp_route(
    namespace: &str,
    name: &str,
    gateway: &str,
    listener_name: &str,
    port: u16,
    listener_key: ListenerKey,
) -> TCPRoute {
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
                name: gateway.to_string(),
                section_name: Some(listener_name.to_string()),
                port: Some(port as i32),
            }]),
            rules: Some(vec![TCPRouteRule {
                backend_refs: Some(vec![TCPBackendRef {
                    name: "test-service".to_string(),
                    namespace: Some(namespace.to_string()),
                    port: Some(8080),
                    weight: Some(1),
                    group: None,
                    kind: None,
                    ref_denied: None,
                }]),
            }]),
            resolved_listeners: Some(vec![listener_key]),
            resolved_listener_attachments: None,
        },
        status: None,
    })
}

#[test]
fn test_tcp_route_full_set() {
    let managers = GlobalTcpRouteManagers::new();
    let key9000 = make_listener_key(9000, "TCP");
    let key9001 = make_listener_key(9001, "TCP");
    let _ = managers.get_or_create_listener_manager(&key9000);
    let _ = managers.get_or_create_listener_manager(&key9001);

    let mut data = HashMap::new();
    let route1 = create_test_tcp_route("default", "route1", "gateway1", "tcp-9000", 9000, key9000.clone());
    let route2 = create_test_tcp_route("default", "route2", "gateway1", "tcp-9001", 9001, key9001.clone());

    data.insert("default/route1".to_string(), Arc::new(route1));
    data.insert("default/route2".to_string(), Arc::new(route2));

    managers.full_set(&data);

    for (key, source) in &data {
        let cached = managers.get_route(key).unwrap();
        assert!(Arc::ptr_eq(source, cached.route()));
    }

    let mgr9000 = managers.get_or_create_listener_manager(&key9000);
    assert!(mgr9000.load_route_table().match_route().is_some());

    let mgr9001 = managers.get_or_create_listener_manager(&key9001);
    assert!(mgr9001.load_route_table().match_route().is_some());

    let key9002 = make_listener_key(9002, "TCP");
    let mgr9002 = managers.get_or_create_listener_manager(&key9002);
    assert!(mgr9002.load_route_table().match_route().is_none());
}

#[test]
fn test_partial_update_shares_schema_and_preserves_prior_generation() {
    let managers = GlobalTcpRouteManagers::new();
    let old_listener = make_listener_key(9400, "TCP");
    let new_listener = make_listener_key(9401, "TCP");
    let source = Arc::new(create_test_tcp_route(
        "default",
        "a-moving",
        "gateway",
        "old",
        9400,
        old_listener.clone(),
    ));
    let sibling_source = Arc::new(create_test_tcp_route(
        "default",
        "z-sibling",
        "gateway",
        "new",
        9401,
        new_listener.clone(),
    ));
    managers.partial_update(
        HashMap::from([
            ("default/a-moving".to_string(), Arc::clone(&source)),
            ("default/z-sibling".to_string(), Arc::clone(&sibling_source)),
        ]),
        HashMap::new(),
        HashSet::new(),
    );
    let prior = managers.get_route("default/a-moving").unwrap();
    let sibling = managers.get_route("default/z-sibling").unwrap();
    assert!(Arc::ptr_eq(&source, prior.route()));
    assert!(Arc::ptr_eq(&sibling_source, sibling.route()));
    let old_manager = managers.get_or_create_listener_manager(&old_listener);
    let old_snapshot = Arc::clone(&old_manager.load_route_table());
    assert!(Arc::ptr_eq(&prior, &old_snapshot.match_route().unwrap()));

    let mut updated = (*source).clone();
    updated.spec.resolved_listeners = Some(vec![new_listener.clone()]);
    let parent = &mut updated.spec.parent_refs.as_mut().unwrap()[0];
    parent.section_name = Some("new".to_string());
    parent.port = Some(9401);
    updated.spec.rules.as_mut().unwrap()[0].backend_refs.as_mut().unwrap()[0].name = "new-service".to_string();
    let updated = Arc::new(updated);
    managers.partial_update(
        HashMap::new(),
        HashMap::from([("default/a-moving".to_string(), Arc::clone(&updated))]),
        HashSet::new(),
    );

    let current = managers.get_route("default/a-moving").unwrap();
    assert!(Arc::ptr_eq(&updated, current.route()));
    assert!(!Arc::ptr_eq(&prior, &current));
    assert!(old_manager.load_route_table().match_route().is_none());
    let new_manager = managers.get_or_create_listener_manager(&new_listener);
    assert!(Arc::ptr_eq(
        &current,
        &new_manager.load_route_table().match_route().unwrap()
    ));
    assert_eq!(
        current.select_backend().unwrap().service().unwrap().name(),
        "new-service"
    );

    // Retained input and in-flight snapshots keep the previous generation.
    assert_eq!(resolved_listeners_for_route(&source), &[old_listener]);
    assert!(Arc::ptr_eq(&prior, &old_snapshot.match_route().unwrap()));
    assert!(Arc::ptr_eq(&source, prior.route()));
    assert_eq!(
        prior.select_backend().unwrap().service().unwrap().name(),
        "test-service"
    );
    let sibling_after = managers.get_route("default/z-sibling").unwrap();
    assert!(Arc::ptr_eq(&sibling, &sibling_after));
    assert!(Arc::ptr_eq(
        sibling.backend_generation(),
        sibling_after.backend_generation()
    ));
}

#[test]
fn test_misclassified_add_rebuilds_old_listener() {
    // Simulates upstream misclassification: an existing key is sent in
    // the `add` bucket instead of `update`. Pre-collection of affected
    // listeners must include add.keys() so the old listener is scheduled
    // for rebuild when the route migrates to a new listener.
    let managers = GlobalTcpRouteManagers::new();
    let key9000 = make_listener_key(9000, "TCP");
    let key9001 = make_listener_key(9001, "TCP");

    // Setup: route-x on L9000
    let mut add = HashMap::new();
    add.insert(
        "default/route-x".to_string(),
        Arc::new(create_test_tcp_route(
            "default",
            "route-x",
            "gw1",
            "tcp-9000",
            9000,
            key9000.clone(),
        )),
    );
    managers.partial_update(add, HashMap::new(), HashSet::new());
    let mgr9000 = managers.get_or_create_listener_manager(&key9000);
    assert!(
        mgr9000.load_route_table().match_route().is_some(),
        "test setup failed: route-x should be in L9000"
    );

    // Misclassified add: same key, new version bound to L9001
    let mut misclassified_add = HashMap::new();
    misclassified_add.insert(
        "default/route-x".to_string(),
        Arc::new(create_test_tcp_route(
            "default",
            "route-x",
            "gw1",
            "tcp-9001",
            9001,
            key9001.clone(),
        )),
    );
    managers.partial_update(misclassified_add, HashMap::new(), HashSet::new());

    // L9000 must be rebuilt: route-x should be gone
    assert!(
        mgr9000.load_route_table().match_route().is_none(),
        "Bug 2: L9000 still has stale route after misclassified add"
    );
    // L9001 should have the new route (created lazily during partial_update)
    let mgr9001 = managers.get_or_create_listener_manager(&key9001);
    assert!(
        mgr9001.load_route_table().match_route().is_some(),
        "Bug 2 (corollary): L9001 should have the new route"
    );
}

#[test]
fn invalid_source_withdraws_units_listener_and_publication_and_repairs_bare_key() {
    use crate::plugins::publication::inputs::GatewayInput;
    use edgion_resources::ResourceMeta;
    use serde_json::json;
    for operation in ["full", "update", "add", "overlap"] {
        let publisher = PluginPublisher::new();
        publisher.replace_gateway_classes(HashMap::from([(
            "class".into(),
            Arc::new(
                serde_json::from_value(json!({
                    "metadata": {"name": "class"}, "spec": {"controllerName": "edgion.io/gateway-controller"}
                }))
                .unwrap(),
            ),
        )]));
        let gateway: edgion_resources::Gateway = crate::test_current_status::from_value(json!({
            "metadata": {"name": "gateway", "namespace": "default"},
            "spec": {"gatewayClassName": "class", "listeners": [{"name": "tcp", "port": 19876, "protocol": "TCP"}]}
        }))
        .unwrap();
        publisher.replace_gateways(HashMap::from([(
            "default/gateway".into(),
            GatewayInput::from_gateway(&gateway),
        )]));
        publisher.replace_stream_plugins(HashMap::from([("default/policy".into(), Arc::new(serde_json::from_value(json!({
            "metadata": {"name": "policy", "namespace": "default"},
            "spec": {"plugins": [{"type": "IpRestriction", "config": {"denyRefs": [{"name": "blocked-networks"}]}}]}
        })).unwrap()))]));
        assert_eq!(publisher.snapshot().stats().stream_cached_entries, 0);
        let managers = GlobalTcpRouteManagers::new();
        let listener = make_listener_key(19876, "TCP");
        let mut route = create_test_tcp_route("default", "route", "gateway", "tcp", 19876, listener.clone());
        route.metadata.namespace = None;
        route.metadata.annotations = Some(std::collections::BTreeMap::from([(
            "edgion.io/edgion-stream-plugins".into(),
            "policy".into(),
        )]));
        route.spec.rules.as_mut().unwrap()[0].backend_refs.as_mut().unwrap()[0].namespace = None;
        let key = route.key_name();
        assert_eq!(
            key, "route",
            "manager identity stays raw while target defaults to default"
        );
        let valid = Arc::new(route.clone());
        route.metadata.name = Some(String::new());
        let invalid = Arc::new(route);
        managers.full_set_with_publisher(&HashMap::from([(key.clone(), invalid.clone())]), &publisher);
        assert!(managers.get_route(&key).is_none());
        assert_eq!(
            publisher.snapshot().stats().stream_cached_entries,
            0,
            "invalid owner creates no executable plugin demand"
        );
        managers.full_set_with_publisher(&HashMap::from([(key.clone(), valid.clone())]), &publisher);
        let original = managers.get_route(&key).unwrap();
        assert!(managers.get_route("default/route").is_none());
        assert!(Arc::ptr_eq(original.route(), &valid));
        assert_eq!(
            original.select_backend().unwrap().service().unwrap().key(),
            "default/test-service"
        );
        assert_eq!(
            publisher.snapshot().stats().stream_cached_entries,
            1,
            "positive control compiles the demanded stream entry"
        );
        assert!(publisher
            .snapshot()
            .gateway("default/gateway")
            .unwrap()
            .scope
            .stream("default/policy")
            .is_some());
        let retained_manager = managers.get_or_create_listener_manager(&listener);
        let table = retained_manager.load_route_table();
        assert!(table.match_route().is_some());
        drop(table);
        match operation {
            "full" => managers.full_set_with_publisher(&HashMap::from([(key.clone(), invalid)]), &publisher),
            "update" => managers.partial_update_with_publisher(
                HashMap::new(),
                HashMap::from([(key.clone(), invalid)]),
                HashSet::new(),
                &publisher,
            ),
            "add" => managers.partial_update_with_publisher(
                HashMap::from([(key.clone(), invalid)]),
                HashMap::new(),
                HashSet::new(),
                &publisher,
            ),
            "overlap" => managers.partial_update_with_publisher(
                HashMap::from([(key.clone(), invalid)]),
                HashMap::from([(key.clone(), valid.clone())]),
                HashSet::from([key.clone()]),
                &publisher,
            ),
            _ => unreachable!(),
        }
        assert!(managers.get_route(&key).is_none());
        let table = retained_manager.load_route_table();
        assert!(table.match_route().is_none(), "current retained listener must be empty");
        drop(table);
        assert_eq!(publisher.snapshot().stats().stream_cached_entries, 0);
        assert!(publisher
            .snapshot()
            .gateway("default/gateway")
            .unwrap()
            .scope
            .stream("default/policy")
            .is_none());
        assert_eq!(
            original.select_backend().unwrap().service().unwrap().key(),
            "default/test-service",
            "old selected generation remains immutable"
        );
        managers.partial_update_with_publisher(
            HashMap::new(),
            HashMap::from([(key.clone(), valid)]),
            HashSet::new(),
            &publisher,
        );
        let repaired = managers.get_route(&key).unwrap();
        assert!(!Arc::ptr_eq(&original, &repaired));
        assert_eq!(publisher.snapshot().stats().stream_cached_entries, 1);
        let repaired_manager = managers.get_or_create_listener_manager(&listener);
        managers.partial_update_with_publisher(
            HashMap::new(),
            HashMap::new(),
            HashSet::from([key.clone()]),
            &publisher,
        );
        assert!(managers.get_route(&key).is_none());
        let table = repaired_manager.load_route_table();
        assert!(table.match_route().is_none());
        assert_eq!(publisher.snapshot().stats().stream_cached_entries, 0);
    }
}
