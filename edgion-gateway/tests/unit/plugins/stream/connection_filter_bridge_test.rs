use super::*;
use crate::plugins::stream::plugin_generation::{StreamPluginGeneration, StreamStages};
use edgion_resources::resources::plugin_policy::PluginPolicyConfig;

#[test]
fn peer_ip_canonicalization_preserves_native_addresses_and_folds_mapped_ipv4() {
    let ipv4: SocketAddr = "192.0.2.1:443".parse().unwrap();
    let mapped: SocketAddr = "[::ffff:192.0.2.1]:443".parse().unwrap();
    let ipv6: SocketAddr = "[2001:db8::1]:443".parse().unwrap();

    assert_eq!(canonical_peer_ip(&ipv4), "192.0.2.1".parse::<IpAddr>().unwrap());
    assert_eq!(canonical_peer_ip(&mapped), "192.0.2.1".parse::<IpAddr>().unwrap());
    assert_eq!(canonical_peer_ip(&ipv6), "2001:db8::1".parse::<IpAddr>().unwrap());
}

#[tokio::test]
async fn unavailable_policy_denies_even_without_peer_or_attachment() {
    assert!(!accept_connection_view(ConnectionView::Unavailable, None, 18092).await);
    let peer = "10.0.0.1:54321".parse().unwrap();
    assert!(!accept_connection_view(ConnectionView::Unavailable, Some(&peer), 18092).await);
    assert!(accept_connection_view(ConnectionView::Empty, Some(&peer), 18092).await);
}

#[tokio::test]
async fn pinned_connection_views_preserve_port_budgets_and_release() {
    let source = Arc::new(serde_yaml::from_str(
            "apiVersion: edgion.io/v1\nkind: EdgionStreamPlugins\nmetadata: {name: bridge-generation, namespace: default}\nspec:\n  plugins:\n    - type: ConnectionRateLimit\n      config:\n        perListener: {rate: 1, interval: 1h}\n"
        ).unwrap());
    let mut generation = StreamPluginGeneration::new(source);
    let policy = PluginPolicyConfig::default().compile().unwrap();
    let stages = StreamStages {
        connection: true,
        tls_route: false,
    };
    let view = generation.compile(&policy, stages);
    let weak = Arc::downgrade(&view);
    let peer = "10.0.0.1:54321".parse().unwrap();
    assert!(accept_connection_view(ConnectionView::Ready(Arc::clone(&view)), Some(&peer), 18092).await);
    assert!(!accept_connection_view(ConnectionView::Ready(Arc::clone(&view)), Some(&peer), 18092).await);
    assert!(accept_connection_view(ConnectionView::Ready(Arc::clone(&view)), Some(&peer), 18093).await);
    assert!(!accept_connection_view(ConnectionView::Unavailable, Some(&peer), 18092).await);
    drop(view);
    assert!(weak.upgrade().is_none());
}
#[test]
fn stream_parent_ports_preserve_scope_across_gateway_publications() {
    use crate::plugins::publication::{inputs::GatewayInput, PluginPublisher};
    use crate::routes::{tcp::TcpRouteUnit, udp::UdpRouteUnit};
    use serde_json::json;
    use std::collections::{HashMap, HashSet};

    for (protocol, kind) in [("TCP", "TCPRoute"), ("UDP", "UDPRoute")] {
        let publisher = PluginPublisher::new();
        publisher.replace_gateway_classes(
            ["ready-a", "ready-b"]
                .into_iter()
                .map(|name| {
                    let class = Arc::new(
                        serde_json::from_value(json!({
                            "apiVersion": "gateway.networking.k8s.io/v1", "kind": "GatewayClass",
                            "metadata": {"name": name},
                            "spec": {"controllerName": "edgion.io/gateway-controller"}
                        }))
                        .unwrap(),
                    );
                    (name.into(), class)
                })
                .collect(),
        );
        let gateway = |name: &str, ports: &[i32]| {
            let gateway: edgion_resources::Gateway = crate::test_current_status::from_value(json!({
                "apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
                "metadata": {"namespace": "default", "name": name},
                "spec": {"gatewayClassName": format!("ready-{name}"), "listeners": ports.iter().map(|port|
                    json!({"name": format!("stream-{port}"), "port": port, "protocol": protocol})
                ).collect::<Vec<_>>()}
            }))
            .unwrap();
            GatewayInput::from_gateway(&gateway)
        };
        let original = HashMap::from([
            ("default/a".into(), gateway("a", &[8080, 8081])),
            ("default/b".into(), gateway("b", &[8081])),
        ]);
        publisher.replace_gateways(original.clone());
        let route = json!({
            "apiVersion": "gateway.networking.k8s.io/v1", "kind": kind,
            "metadata": {"namespace": "default", "name": "port-owner"},
            "spec": {"resolvedStatusController": "edgion.io/gateway-controller",
                "parentRefs": [{"name": "a", "port": 8080}, {"name": "b", "port": 8081}],
                "rules": [{"backendRefs": [{"name": "svc", "port": 80}]}]}
        });
        let parents = match kind {
            "TCPRoute" => TcpRouteUnit::from_route(Arc::new(crate::test_current_status::from_value(route).unwrap()))
                .unwrap()
                .resolved_parent_refs()
                .to_vec(),
            "UDPRoute" => UdpRouteUnit::from_route(Arc::new(crate::test_current_status::from_value(route).unwrap()))
                .unwrap()
                .resolved_parent_refs()
                .to_vec(),
            _ => unreachable!(),
        };
        let first_key = ListenerKey::new("0.0.0.0:8080".parse().unwrap(), protocol);
        let shared_key = ListenerKey::new("0.0.0.0:8081".parse().unwrap(), protocol);
        let old = publisher.snapshot();
        let first_scope = resolve_route_scope(&old, &first_key, &parents, "", "default", kind).unwrap();
        let shared_scope = resolve_route_scope(&old, &shared_key, &parents, "", "default", kind).unwrap();
        assert!(Arc::ptr_eq(&first_scope, &old.gateway("default/a").unwrap().scope));
        assert!(Arc::ptr_eq(&shared_scope, &old.gateway("default/b").unwrap().scope));
        assert!(!Arc::ptr_eq(&first_scope, &shared_scope));

        // Moving B leaves only A's unreferenced 8081 listener on the shared socket.
        publisher.update_gateways(
            HashMap::from([("default/b".into(), gateway("b", &[9090]))]),
            &HashSet::new(),
        );
        let moved = publisher.snapshot();
        assert!(resolve_route_scope(&moved, &shared_key, &parents, "", "default", kind).is_none());
        let moved_key = ListenerKey::new("0.0.0.0:9090".parse().unwrap(), protocol);
        assert!(resolve_route_scope(&moved, &moved_key, &parents, "", "default", kind).is_none());
        assert!(Arc::ptr_eq(
            &shared_scope,
            &resolve_route_scope(&old, &shared_key, &parents, "", "default", kind).unwrap(),
        ));

        publisher.replace_gateways(original);
        let restored = publisher.snapshot();
        assert!(Arc::ptr_eq(
            &resolve_route_scope(&restored, &shared_key, &parents, "", "default", kind).unwrap(),
            &restored.gateway("default/b").unwrap().scope,
        ));
        assert!(resolve_route_scope(&moved, &shared_key, &parents, "", "default", kind).is_none());
    }
}

#[test]
fn logical_owner_and_policy_are_selected_from_one_publication() {
    use crate::plugins::publication::{
        inputs::{GatewayInput, GatewayListener},
        PluginPublisher,
    };
    use std::collections::{HashMap, HashSet};
    let publisher = PluginPublisher::new();
    let class = Arc::new(serde_yaml::from_str(
            "apiVersion: gateway.networking.k8s.io/v1\nkind: GatewayClass\nmetadata: {name: ready}\nspec: {controllerName: edgion.io/gateway-controller}"
        ).unwrap());
    publisher.replace_gateway_classes(HashMap::from([("ready".into(), class)]));
    let listener = ListenerKey::new("0.0.0.0:18443".parse().unwrap(), "TLS");
    let input = |class: &str| GatewayInput {
        class_name: class.into(),
        http_keys: vec![],
        connection_key: None,
        listeners: vec![GatewayListener {
            key: listener.clone(),
            port: 18443,
            name: "tls".into(),
            hostname: Some("owner.example.com".into()),
            allowed_routes: None,
        }],
    };
    publisher.replace_gateways(HashMap::from([
        ("default/a".into(), input("missing")),
        ("default/b".into(), input("ready")),
    ]));
    let parent = |key: &str| ResolvedParentRef {
        gateway_key: key.into(),
        port: None,
        parent_ns: "default".into(),
        section_name: Some("tls".into()),
    };
    let old = publisher.snapshot();
    let b = [parent("default/b")];
    let scope = resolve_route_scope(&old, &listener, &b, "owner.example.com", "default", "TLSRoute").unwrap();
    assert!(resolve_route_scope(
        &old,
        &listener,
        &[parent("default/a")],
        "owner.example.com",
        "default",
        "TLSRoute"
    )
    .is_none());
    assert!(resolve_route_scope(&old, &listener, &b, "other.example.com", "default", "TLSRoute").is_none());
    assert!(resolve_route_scope(&old, &listener, &b, "owner.example.com", "other", "TLSRoute").is_none());
    let mut wrong_section = parent("default/b");
    wrong_section.section_name = Some("other".into());
    assert!(resolve_route_scope(
        &old,
        &listener,
        &[wrong_section],
        "owner.example.com",
        "default",
        "TLSRoute"
    )
    .is_none());
    // A class rebind invalidates new admissions; the old admission remains pinned.
    publisher.update_gateways(HashMap::from([("default/b".into(), input("missing"))]), &HashSet::new());
    assert!(resolve_route_scope(
        &publisher.snapshot(),
        &listener,
        &b,
        "owner.example.com",
        "default",
        "TLSRoute"
    )
    .is_none());
    assert!(Arc::ptr_eq(
        &scope,
        &resolve_route_scope(&old, &listener, &b, "owner.example.com", "default", "TLSRoute").unwrap()
    ));
}
#[tokio::test]
async fn listener_annotation_can_be_added_and_removed_after_startup() {
    use crate::plugins::publication::{inputs::GatewayInput, PluginPublisher};
    use std::collections::{HashMap, HashSet};
    let publisher = PluginPublisher::new();
    let gateway: edgion_resources::Gateway = serde_yaml::from_str(
            "apiVersion: gateway.networking.k8s.io/v1\nkind: Gateway\nmetadata: {name: dynamic, namespace: default}\nspec:\n  gatewayClassName: dynamic\n  listeners: [{name: tcp, port: 18094, protocol: TCP}]"
        ).unwrap();
    let gateway = crate::test_current_status::processed(gateway);
    let class = Arc::new(serde_yaml::from_str(
            "apiVersion: gateway.networking.k8s.io/v1\nkind: GatewayClass\nmetadata: {name: dynamic}\nspec: {controllerName: edgion.io/gateway-controller}"
        ).unwrap());
    publisher.replace_gateway_classes(HashMap::from([("dynamic".into(), class)]));
    let mut input = GatewayInput::from_gateway(&gateway);
    let key = input.listeners[0].key.clone();
    let peer = "10.0.0.1:54321".parse().unwrap();
    publisher.replace_gateways(HashMap::from([("default/dynamic".into(), input.clone())]));
    assert!(accept_connection_view(publisher.snapshot().connection(&key), Some(&peer), 18094).await);
    input.connection_key = Some("default/dynamic-policy".into());
    publisher.update_gateways(
        HashMap::from([("default/dynamic".into(), input.clone())]),
        &HashSet::new(),
    );
    assert!(!accept_connection_view(publisher.snapshot().connection(&key), Some(&peer), 18094).await);
    let source = Arc::new(serde_yaml::from_str(
            "apiVersion: edgion.io/v1\nkind: EdgionStreamPlugins\nmetadata: {name: dynamic-policy, namespace: default}\nspec:\n  plugins:\n    - type: IpRestriction\n      config:\n        deny: [{name: blocked, cidrs: [10.0.0.0/8]}]"
        ).unwrap());
    publisher.replace_stream_plugins(HashMap::from([("default/dynamic-policy".into(), source)]));
    assert!(!accept_connection_view(publisher.snapshot().connection(&key), Some(&peer), 18094).await);
    input.connection_key = None;
    publisher.update_gateways(HashMap::from([("default/dynamic".into(), input)]), &HashSet::new());
    assert!(accept_connection_view(publisher.snapshot().connection(&key), Some(&peer), 18094).await);
    assert_eq!(publisher.snapshot().stats().stream_cached_entries, 0);
}
