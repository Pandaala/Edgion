use super::*;

struct RecordingHandler(Arc<RwLock<HashMap<String, Arc<TCPRoute>>>>);

impl ConfHandler<TCPRoute> for RecordingHandler {
    fn full_set(&self, data: &HashMap<String, Arc<TCPRoute>>) {
        *self.0.write() = data.clone();
    }

    fn partial_update(
        &self,
        add: HashMap<String, Arc<TCPRoute>>,
        update: HashMap<String, Arc<TCPRoute>>,
        remove: HashSet<String>,
    ) {
        let mut current = self.0.write();
        for key in remove {
            current.remove(&key);
        }
        current.extend(add);
        current.extend(update);
    }
}

#[test]
fn shared_scope_change_reprojects_existing_l4_route() {
    let route: TCPRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "shared", "namespace": "routes"},
        "spec": {
            "parentRefs": [{"name": "edge-a", "namespace": "infra"}, {"name": "edge-b", "namespace": "infra"}],
            "resolvedListeners": ["0.0.0.0:80/TCP", "0.0.0.0:81/TCP"],
            "resolvedListenerAttachments": [
                {"gatewayNamespace": "infra", "gatewayName": "edge-a", "listenerName": "tcp-a", "listenerKey": "0.0.0.0:80/TCP"},
                {"gatewayNamespace": "infra", "gatewayName": "edge-b", "listenerName": "tcp-b", "listenerKey": "0.0.0.0:81/TCP"}
            ]
        }
    })).unwrap();
    let shared = Arc::new(SharedScopeRegistry::default());
    let recorded = Arc::new(RwLock::new(HashMap::new()));
    let handler = ScopedHandler::wrap(
        Box::new(RecordingHandler(Arc::clone(&recorded))),
        None,
        Arc::clone(&shared),
    );
    handler.full_set(&HashMap::from([("routes/shared".into(), Arc::new(route))]));
    assert_eq!(
        recorded.read()["routes/shared"]
            .spec
            .resolved_listener_attachments
            .as_ref()
            .unwrap()
            .len(),
        2
    );
    shared.replace(HashSet::from(["infra/edge-a".into()]));
    assert_eq!(
        recorded.read()["routes/shared"]
            .spec
            .resolved_listeners
            .as_ref()
            .unwrap(),
        &vec![edgion_resources::ListenerKey("0.0.0.0:81/TCP".into())]
    );
    shared.replace(HashSet::from(["infra/edge-a".into(), "infra/edge-b".into()]));
    assert!(recorded.read().is_empty());
    shared.replace(HashSet::new());
    assert_eq!(
        recorded.read()["routes/shared"]
            .spec
            .resolved_listener_attachments
            .as_ref()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn dedicated_scope_withdraws_routes_when_gateway_uid_is_replaced() {
    let route: TCPRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "edge-route", "namespace": "routes"},
        "spec": {
            "parentRefs": [{"name": "edge", "namespace": "infra"}],
            "resolvedListeners": ["0.0.0.0:80/TCP"],
            "resolvedListenerAttachments": [
                {"gatewayNamespace": "infra", "gatewayName": "edge", "listenerName": "tcp", "listenerKey": "0.0.0.0:80/TCP"}
            ]
        }
    })).unwrap();
    let registry = Arc::new(SharedScopeRegistry::default());
    let recorded = Arc::new(RwLock::new(HashMap::new()));
    let handler = ScopedHandler::wrap(
        Box::new(RecordingHandler(Arc::clone(&recorded))),
        Some("infra/edge".into()),
        Arc::clone(&registry),
    );
    handler.full_set(&HashMap::from([("routes/edge-route".into(), Arc::new(route))]));
    assert!(recorded.read().is_empty());
    registry.set_active(true);
    assert!(recorded.read().contains_key("routes/edge-route"));
    registry.set_active(false);
    assert!(recorded.read().is_empty());
}

#[test]
fn tcp_route_projection_keeps_only_owned_attachment() {
    let route: TCPRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "shared", "namespace": "routes"},
        "spec": {
            "parentRefs": [
                {"name": "edge-a", "namespace": "infra"},
                {"name": "edge-b", "namespace": "infra"}
            ],
            "resolvedListeners": ["0.0.0.0:80/TCP", "0.0.0.0:81/TCP"],
            "resolvedListenerAttachments": [
                {"gatewayNamespace": "infra", "gatewayName": "edge-a", "listenerName": "tcp-a", "listenerKey": "0.0.0.0:80/TCP"},
                {"gatewayNamespace": "infra", "gatewayName": "edge-b", "listenerName": "tcp-b", "listenerKey": "0.0.0.0:81/TCP"}
            ]
        }
    }))
    .unwrap();
    let scoped = route.project_for_gateway("infra/edge-a").unwrap();
    assert_eq!(scoped.spec.parent_refs.unwrap().len(), 1);
    assert_eq!(
        scoped.spec.resolved_listeners.unwrap(),
        vec![edgion_resources::ListenerKey("0.0.0.0:80/TCP".into())]
    );
    assert_eq!(scoped.spec.resolved_listener_attachments.unwrap().len(), 1);
    assert!(route.project_for_gateway("infra/missing").is_none());
}

#[test]
fn tls_route_projection_trims_other_gateway_listener() {
    let route: TLSRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "shared", "namespace": "routes"},
        "spec": {
            "parentRefs": [
                {"name": "edge-a", "namespace": "infra"},
                {"name": "edge-b", "namespace": "infra"}
            ],
            "hostnames": ["example.com"],
            "resolvedListeners": [
                {"gatewayNamespace": "infra", "gatewayName": "edge-a", "listenerName": "tls", "listenerKey": "0.0.0.0:443/TLS", "hostnames": ["example.com"]},
                {"gatewayNamespace": "infra", "gatewayName": "edge-b", "listenerName": "tls", "listenerKey": "0.0.0.0:443/TLS", "hostnames": ["example.com"]}
            ]
        }
    }))
    .unwrap();
    let scoped = route.project_for_gateway("infra/edge-b").unwrap();
    let listener = &scoped.spec.resolved_listeners.unwrap()[0];
    assert_eq!(listener.gateway_name, "edge-b");
}
