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

#[tokio::test]
async fn backend_client_publication_full_and_partial_updates_withdraw_and_recover() {
    crate::test_subprocess::assert_in_subprocess_async("runtime::handler::tests", "backend_client_publication_full_and_partial_updates_withdraw_and_recover", "EDGION_R11_GATEWAY_HANDLER_CHILD", "publication", || async {
    let handler = GatewayHandler::new_scoped(Some("r11-handler/edge".into()));
    let key = "r11-handler/edge".to_string();
    let store = get_global_gateway_backend_tls_store();
    let owner: Gateway = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"edge","namespace":"r11-handler"},"spec":{"gatewayClassName":"public","listeners":[],
        "tls":{"backend":{"clientCertificateRef":{"name":"client"},"resolvedClientCertificate":{"metadata":{"name":"client","namespace":"r11-handler"}}}}}
    })).unwrap();
    let mut invalid = owner.clone();
    invalid
        .spec
        .tls
        .as_mut()
        .unwrap()
        .backend
        .as_mut()
        .unwrap()
        .client_certificate_ref
        .as_mut()
        .unwrap()
        .name = "retargeted".into();
    for candidate in [&owner, &invalid, &owner] {
        handler.full_set(&HashMap::from([(key.clone(), Arc::new(candidate.clone()))]));
        assert_eq!(store.get(&key).is_some(), std::ptr::eq(candidate, &owner));
    }
    handler.full_set(&HashMap::new());
    assert!(store.get(&key).is_none());
    handler.partial_update(
        HashMap::from([(key.clone(), Arc::new(owner.clone()))]),
        HashMap::new(),
        HashSet::new(),
    );
    assert!(store.get(&key).is_some());
    handler.partial_update(
        HashMap::new(),
        HashMap::from([(key.clone(), Arc::new(invalid))]),
        HashSet::new(),
    );
    assert!(store.get(&key).is_none());
    handler.partial_update(
        HashMap::new(),
        HashMap::from([(key.clone(), Arc::new(owner))]),
        HashSet::new(),
    );
    assert!(store.get(&key).is_some());
    handler.partial_update(HashMap::new(), HashMap::new(), HashSet::from([key.clone()]));
    assert!(store.get(&key).is_none());
    }).await;
}

#[tokio::test]
async fn gateway_same_key_admission_refresh_preserves_bound_route_handles() {
    crate::test_subprocess::assert_in_subprocess_async(
        "runtime::handler::tests",
        "gateway_same_key_admission_refresh_preserves_bound_route_handles",
        "EDGION_SHARED_GATEWAY_HANDLER_CHILD",
        "admission",
        || async {
            let handler = GatewayHandler::new_scoped(Some("shared-flow/edge".into()));
            let owner: Gateway = crate::test_current_status::from_value(serde_json::json!({
                "metadata":{"name":"edge","namespace":"shared-flow"},
                "spec":{"gatewayClassName":"public","listeners":[{"name":"http","port":9855,"protocol":"HTTP"}]}
            }))
            .unwrap();
            let key = owner.key_name();
            let listener_key = edgion_resources::ListenerKey::new("0.0.0.0:9855".parse().unwrap(), "HTTP");
            handler.full_set(&HashMap::from([(key.clone(), Arc::new(owner.clone()))]));
            let http = get_global_http_route_managers().get_or_create_listener_manager(&listener_key);
            let grpc = get_global_grpc_route_managers().get_or_create_listener_manager(&listener_key);
            assert!(get_listener_gateway_info_store()
                .get_index(&listener_key)
                .iter_gateways()
                .next()
                .is_some());
            let mut rejected = owner.clone();
            let condition = &mut rejected
                .spec
                .current_status
                .as_mut()
                .unwrap()
                .listeners
                .as_mut()
                .unwrap()[0]
                .conditions[0];
            condition.status = "False".into();
            condition.reason = "SharedListenerPolicyConflict".into();
            handler.partial_update(
                HashMap::new(),
                HashMap::from([(key.clone(), Arc::new(rejected.clone()))]),
                HashSet::new(),
            );
            assert!(get_listener_gateway_info_store()
                .get_index(&listener_key)
                .iter_gateways()
                .next()
                .is_none());
            assert!(Arc::ptr_eq(
                &http,
                &get_global_http_route_managers().get_or_create_listener_manager(&listener_key)
            ));
            assert!(Arc::ptr_eq(
                &grpc,
                &get_global_grpc_route_managers().get_or_create_listener_manager(&listener_key)
            ));
            assert!(http.load_route_table().hosts.is_empty());
            assert!(!grpc.load_route_table().has_routes());
            handler.full_set(&HashMap::from([(key.clone(), Arc::new(owner.clone()))]));
            assert!(get_listener_gateway_info_store()
                .get_index(&listener_key)
                .iter_gateways()
                .next()
                .is_some());
            assert!(Arc::ptr_eq(
                &http,
                &get_global_http_route_managers().get_or_create_listener_manager(&listener_key)
            ));
            assert!(Arc::ptr_eq(
                &grpc,
                &get_global_grpc_route_managers().get_or_create_listener_manager(&listener_key)
            ));
            // A rejected declaration is retained, but true removal must retire its handle.
            for partial_delete in [false, true] {
                handler.full_set(&HashMap::from([(key.clone(), Arc::new(rejected.clone()))]));
                let old_http = get_global_http_route_managers().get_or_create_listener_manager(&listener_key);
                let old_grpc = get_global_grpc_route_managers().get_or_create_listener_manager(&listener_key);
                if partial_delete {
                    handler.partial_update(HashMap::new(), HashMap::new(), HashSet::from([key.clone()]));
                } else {
                    handler.full_set(&HashMap::new());
                }
                handler.full_set(&HashMap::from([(key.clone(), Arc::new(owner.clone()))]));
                assert!(!Arc::ptr_eq(
                    &old_http,
                    &get_global_http_route_managers().get_or_create_listener_manager(&listener_key)
                ));
                assert!(!Arc::ptr_eq(
                    &old_grpc,
                    &get_global_grpc_route_managers().get_or_create_listener_manager(&listener_key)
                ));
            }
            handler.full_set(&HashMap::new());
        },
    )
    .await;
}
