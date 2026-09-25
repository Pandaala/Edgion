use super::*;

#[test]
fn gateway_scope_matches_exact_namespace_and_name() {
    let mut gateway: Gateway = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "edge", "namespace": "one", "uid": "first"},
        "spec": {"gatewayClassName": "public", "listeners": []}
    }))
    .unwrap();
    assert!(gateway_in_scope(Some("one/edge"), Some("first"), "one/edge", &gateway));
    assert!(!gateway_in_scope(
        Some("one/edge"),
        Some("second"),
        "one/edge",
        &gateway
    ));
    assert!(!gateway_in_scope(Some("one/edge"), None, "two/edge", &gateway));
    assert!(!gateway_in_scope(Some("one/edge"), None, "one/edge-other", &gateway));
    assert!(gateway_in_scope(None, None, "one/edge", &gateway));
    gateway.metadata.annotations = Some(std::collections::BTreeMap::from([(
        edgion_resources::constants::annotations::edgion::DEDICATED_GATEWAY_INSTANCE.to_string(),
        "true".to_string(),
    )]));
    assert!(!gateway_in_scope(None, None, "one/edge", &gateway));
}

#[test]
fn repeated_add_replaces_gateway_used_for_rebuild() {
    let mut store = GatewayStore::new();
    let make_gateway = |hostname: &str| -> Gateway {
        serde_json::from_value(serde_json::json!({
            "metadata": {"name": "gateway", "namespace": "apps"},
            "spec": {"gatewayClassName": "public", "listeners": [
                {"name": "web", "port": 8080, "protocol": "HTTP", "hostname": hostname}
            ]}
        }))
        .unwrap()
    };
    let old = make_gateway("old.example.com");
    let new = make_gateway("new.example.com");
    store_added_gateway(&mut store, old.clone());
    assert_eq!(
        serde_json::to_value(store.list_gateways()).unwrap(),
        serde_json::json!([old])
    );

    store_added_gateway(&mut store, new.clone());
    // This is the same snapshot consumed by the handler's derived rebuilds.
    assert_eq!(
        serde_json::to_value(store.list_gateways()).unwrap(),
        serde_json::json!([new])
    );
    assert_eq!(store.count(), 1);
}
