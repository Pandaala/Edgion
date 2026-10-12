use super::*;
use serde_json::json;

fn gateway(name: &str) -> GatewayInput {
    let source = crate::test_current_status::from_value::<edgion_resources::Gateway>(json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
        "metadata": {"namespace": "apps", "name": name},
        "spec": {"gatewayClassName": "class", "listeners": [{"name": "web", "port": 18080, "protocol": "HTTP"}]}
    }))
    .unwrap();
    GatewayInput::from_gateway(&source)
}

#[test]
fn epoch_stamping_reused_shared_listener_preserves_tables_indices_and_scopes() {
    let publisher = PluginPublisher::new();
    publisher.replace_gateways(HashMap::from([
        ("apps/a".into(), gateway("a")),
        ("apps/b".into(), gateway("b")),
    ]));
    let key = ListenerKey("0.0.0.0:18080/HTTP".into());
    let original = publisher.snapshot();
    let before = original.http_listener(&key);
    assert_eq!(before.gateway_count(), 2);
    let mut next = publisher.state.lock().compose(&original);
    assert!(
        Arc::ptr_eq(&before, &next.http_listener(&key)),
        "composition reuses unchanged listener"
    );
    let mut first = crate::response_cache::PublicationEpoch::default();
    first.id = 101;
    next.stamp_cache_epoch(first);
    let after = next.http_listener(&key);
    assert!(!Arc::ptr_eq(&before, &after));
    assert_eq!(after.cache_epoch.id, 101);
    assert_eq!(next.cache_epoch.id, 101);
    assert!(Arc::ptr_eq(&before.http, &after.http));
    assert!(Arc::ptr_eq(&before.grpc, &after.grpc));
    assert!(Arc::ptr_eq(&before.index, &after.index));
    for name in ["apps/a", "apps/b"] {
        let old_child = before.select_gateway(name).unwrap();
        let new_child = after.select_gateway(name).unwrap();
        assert_eq!(new_child.cache_epoch.id, 101);
        assert!(Arc::ptr_eq(&old_child.http, &new_child.http));
        assert!(Arc::ptr_eq(&old_child.grpc, &new_child.grpc));
        assert!(Arc::ptr_eq(&old_child.index, &new_child.index));
        assert!(Arc::ptr_eq(
            old_child.unique_gateway().unwrap(),
            new_child.unique_gateway().unwrap()
        ));
    }
    let mut second = crate::response_cache::PublicationEpoch::default();
    second.id = 102;
    next.stamp_cache_epoch(second);
    assert_eq!(next.http_listener(&key).cache_epoch.id, 102);
    assert_eq!(after.cache_epoch.id, 101, "captured view remains immutable");
}
