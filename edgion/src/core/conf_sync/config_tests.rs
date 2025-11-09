#![cfg(test)]

use crate::core::conf_sync::cache_diff::diff_center_hub;
use crate::core::conf_sync::server_cache::{EventType, Versionable, WatchResponse};
use crate::core::conf_sync::config_server::ConfigServer;
use crate::core::conf_sync::config_client::ConfigClient;
use crate::core::conf_sync::traits::{EventDispatcher, ResourceChange};
use crate::core::conf_sync::{ServerCache, EventDispatch, ClientCache};
use crate::types::{
    EdgionGatewayConfig, EdgionGatewayConfigSpec, EdgionTls, EdgionTlsSpec, HTTPRoute,
    HTTPRouteSpec, ParentReference, ResourceKind,
};
use k8s_openapi::api::core::v1::SecretReference;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::task::yield_now;
use tokio::time::{sleep, timeout, Duration};

const GATEWAY_CLASS_KEY: &str = "test-gateway-class";
const GATEWAY_NAMESPACE: &str = "default";

fn http_route_value(hostname: &str, resource_version: u64) -> Value {
    json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "HTTPRoute",
        "metadata": {
            "name": "demo-route",
            "namespace": "default",
            "resourceVersion": resource_version.to_string()
        },
        "spec": {
            "parentRefs": [
                {
                    "name": GATEWAY_CLASS_KEY,
                    "namespace": "default",
                    "kind": "Gateway",
                    "port": 80
                }
            ],
            "hostnames": [hostname]
        }
    })
}

fn edgion_gateway_config_value(resource_version: u64) -> Value {
    let spec = EdgionGatewayConfigSpec {
        listener_defaults: None,
        load_balancing: None,
        access_log: None,
        security: None,
        limits: None,
        observability: None,
    };
    let mut config = EdgionGatewayConfig::new(GATEWAY_CLASS_KEY, spec);
    config.metadata.resource_version = Some(resource_version.to_string());
    serde_json::to_value(&config).expect("serialize EdgionGatewayConfig")
}

fn resource_fixtures() -> Vec<(ResourceKind, Value, u64)> {
    let tls_spec = EdgionTlsSpec {
        parent_refs: None,
        hosts: vec!["example.com".to_string()],
        secret_ref: SecretReference {
            name: Some("demo-secret".to_string()),
            namespace: Some("default".to_string()),
        },
        certificate_refs: None,
    };

    let mut tls = EdgionTls::new("demo-tls", tls_spec);
    tls.metadata.namespace = Some("default".to_string());
    tls.metadata.resource_version = Some("7".to_string());
    let tls_value = serde_json::to_value(&tls).expect("serialize edgion tls");

    vec![
        (
            ResourceKind::GatewayClass,
            json!({
                "apiVersion": "gateway.networking.k8s.io/v1",
                "kind": "GatewayClass",
                "metadata": {
                    "name": GATEWAY_CLASS_KEY,
                    "resourceVersion": "1"
                },
                "spec": {
                    "controllerName": "example.com/controller"
                }
            }),
            1_u64,
        ),
        (
            ResourceKind::EdgionGatewayConfig,
            edgion_gateway_config_value(2),
            2_u64,
        ),
        (
            ResourceKind::Gateway,
            json!({
                "apiVersion": "gateway.networking.k8s.io/v1",
                "kind": "Gateway",
                "metadata": {
                    "name": GATEWAY_CLASS_KEY,
                    "namespace": "default",
                    "resourceVersion": "3"
                },
                "spec": {
                    "gatewayClassName": GATEWAY_CLASS_KEY,
                    "listeners": [
                        {
                            "name": "http",
                            "port": 80,
                            "protocol": "HTTP"
                        }
                    ]
                }
            }),
            3_u64,
        ),
        (
            ResourceKind::HTTPRoute,
            http_route_value("example.com", 4),
            4_u64,
        ),
        (
            ResourceKind::Service,
            json!({
                "apiVersion": "v1",
                "kind": "Service",
                "metadata": {
                    "name": "demo-service",
                    "namespace": "default",
                    "resourceVersion": "5"
                },
                "spec": {
                    "ports": [
                        {
                            "name": "http",
                            "port": 80,
                            "protocol": "TCP"
                        }
                    ]
                }
            }),
            5_u64,
        ),
        (
            ResourceKind::EndpointSlice,
            json!({
                "apiVersion": "discovery.k8s.io/v1",
                "kind": "EndpointSlice",
                "metadata": {
                    "name": "demo-slice",
                    "namespace": "default",
                    "resourceVersion": "6"
                },
                "addressType": "IPv4",
                "endpoints": [],
                "ports": []
            }),
            6_u64,
        ),
        (ResourceKind::EdgionTls, tls_value, 7_u64),
        (
            ResourceKind::Secret,
            json!({
                "apiVersion": "v1",
                "kind": "Secret",
                "metadata": {
                    "name": "demo-secret",
                    "namespace": "default",
                    "resourceVersion": "8"
                },
                "stringData": {
                    "tls.crt": "dummy-cert",
                    "tls.key": "dummy-key"
                }
            }),
            8_u64,
        ),
    ]
}

fn make_http_route(name: &str, host: &str, version: u64) -> HTTPRoute {
    let spec = HTTPRouteSpec {
        parent_refs: Some(vec![ParentReference {
            group: None,
            kind: Some("Gateway".to_string()),
            namespace: Some(GATEWAY_NAMESPACE.to_string()),
            name: GATEWAY_CLASS_KEY.to_string(),
            section_name: Some("http".to_string()),
            port: Some(80),
        }]),
        hostnames: Some(vec![host.to_string()]),
        rules: None,
    };

    let mut route = HTTPRoute::new(name, spec);
    route.metadata.namespace = Some(GATEWAY_NAMESPACE.to_string());
    route.metadata.resource_version = Some(version.to_string());
    route
}

fn seed_config_server(center: &mut ConfigServer) {
    for (kind, value, version) in resource_fixtures() {
        let data = serde_json::to_string(&value).expect("serialize resource");
        <ConfigServer as EventDispatcher>::apply_resource_change(
            center,
            ResourceChange::EventAdd,
            Some(kind),
            data,
            Some(version),
        );
    }
}

fn extract_route_host(route: &HTTPRoute) -> Option<&str> {
    route
        .spec
        .hostnames
        .as_ref()
        .and_then(|hosts| hosts.first())
        .map(|s| s.as_str())
}

async fn assert_http_route_state(
    center: &ConfigServer,
    hub: &ConfigClient,
    expected_host: Option<&str>,
    expected_version: u64,
    expected_count: usize,
) {
    let list = center
        .list(&GATEWAY_CLASS_KEY.to_string(), &ResourceKind::HTTPRoute)
        .await
        .unwrap_or_else(|e| panic!("list HTTPRoute from center failed: {}", e));
    let routes: Vec<HTTPRoute> = serde_json::from_str(&list.data)
        .unwrap_or_else(|e| panic!("parse HTTPRoute list from center failed: {}", e));
    assert_eq!(
        routes.len(),
        expected_count,
        "center HTTPRoute count mismatch"
    );
    assert_eq!(
        routes.first().and_then(extract_route_host),
        expected_host,
        "center HTTPRoute host mismatch"
    );
    assert_eq!(
        list.resource_version, expected_version,
        "center HTTPRoute resourceVersion mismatch"
    );

    let hub_list = hub.list_routes();
    assert_eq!(
        hub_list.data.len(),
        expected_count,
        "hub HTTPRoute count mismatch"
    );
    assert_eq!(
        hub_list
            .data
            .first()
            .and_then(|route| extract_route_host(*route)),
        expected_host,
        "hub HTTPRoute host mismatch"
    );
    assert_eq!(
        hub_list.resource_version, expected_version,
        "hub HTTPRoute resourceVersion mismatch"
    );
}

async fn exercise_http_route_lifecycle(center: &mut ConfigServer, hub: &mut ConfigClient) {
    let version = 10_u64;

    // Add
    let add_route = http_route_value("example.com", version);
    let add_data = serde_json::to_string(&add_route).expect("serialize HTTPRoute add payload");
    <ConfigServer as EventDispatcher>::apply_resource_change(
        center,
        ResourceChange::EventAdd,
        Some(ResourceKind::HTTPRoute),
        add_data.clone(),
        Some(version),
    );
    sleep(Duration::from_secs(1)).await;
    <ConfigClient as EventDispatcher>::apply_resource_change(
        hub,
        ResourceChange::EventAdd,
        Some(ResourceKind::HTTPRoute),
        add_data.clone(),
        Some(version),
    );
    assert_http_route_state(center, hub, Some("example.com"), version, 1).await;

    // Update
    let update_route = http_route_value("api.example.com", version);
    let update_data =
        serde_json::to_string(&update_route).expect("serialize HTTPRoute update payload");
    <ConfigServer as EventDispatcher>::apply_resource_change(
        center,
        ResourceChange::EventUpdate,
        Some(ResourceKind::HTTPRoute),
        update_data.clone(),
        Some(version),
    );
    sleep(Duration::from_secs(1)).await;
    <ConfigClient as EventDispatcher>::apply_resource_change(
        hub,
        ResourceChange::EventUpdate,
        Some(ResourceKind::HTTPRoute),
        update_data.clone(),
        Some(version),
    );
    assert_http_route_state(center, hub, Some("api.example.com"), version, 1).await;

    // Delete
    <ConfigServer as EventDispatcher>::apply_resource_change(
        center,
        ResourceChange::EventDelete,
        Some(ResourceKind::HTTPRoute),
        update_data.clone(),
        Some(version),
    );
    sleep(Duration::from_secs(1)).await;
    <ConfigClient as EventDispatcher>::apply_resource_change(
        hub,
        ResourceChange::EventDelete,
        Some(ResourceKind::HTTPRoute),
        update_data,
        Some(version),
    );
    assert_http_route_state(center, hub, None, version, 0).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn config_server_data_syncs_into_config_client() {
    let mut config_server = ConfigServer::new();
    seed_config_server(&mut config_server);

    // Allow any spawned tasks to update internal stores
    yield_now().await;
    sleep(Duration::from_millis(5)).await;

    let mut config_client = ConfigClient::new(GATEWAY_CLASS_KEY.to_string());
    let key = GATEWAY_CLASS_KEY.to_string();
    let resource_kinds = [
        ResourceKind::GatewayClass,
        ResourceKind::EdgionGatewayConfig,
        ResourceKind::Gateway,
        ResourceKind::HTTPRoute,
        ResourceKind::Service,
        ResourceKind::EndpointSlice,
        ResourceKind::EdgionTls,
        ResourceKind::Secret,
    ];

    for kind in resource_kinds {
        let list_response = config_server
            .list(&key, &kind)
            .await
            .unwrap_or_else(|e| panic!("list {:?} from center failed: {}", kind, e));

        let resources: Vec<Value> = serde_json::from_str(&list_response.data)
            .unwrap_or_else(|e| panic!("parse list data for {:?} failed: {}", kind, e));

        for resource in resources {
            let data = serde_json::to_string(&resource).expect("serialize resource for hub");
            <ConfigClient as EventDispatcher>::apply_resource_change(
                &mut config_client,
                ResourceChange::InitAdd,
                Some(kind),
                data,
                Some(list_response.resource_version),
            );
        }
    }

    let gateway_classes = config_client.list_gateway_classes();
    assert_eq!(gateway_classes.data.len(), 1);
    assert_eq!(gateway_classes.resource_version, 1);
    assert_eq!(
        gateway_classes.data[0].metadata.name.as_deref(),
        Some(GATEWAY_CLASS_KEY)
    );

    let gateway_configs = config_client.list_edgion_gateway_config();
    assert_eq!(gateway_configs.data.len(), 1);
    assert_eq!(gateway_configs.resource_version, 2);
    assert_eq!(
        gateway_configs.data[0].metadata.name.as_deref(),
        Some(GATEWAY_CLASS_KEY)
    );
    assert!(
        gateway_configs.data[0].spec.listener_defaults.is_none(),
        "expected listener_defaults to be None"
    );

    let gateways = config_client.list_gateways();
    assert_eq!(gateways.data.len(), 1);
    assert_eq!(gateways.resource_version, 3);
    assert_eq!(
        gateways.data[0].metadata.name.as_deref(),
        Some(GATEWAY_CLASS_KEY)
    );
    assert_eq!(
        gateways.data[0].spec.listeners.as_ref().map(|l| l[0].port),
        Some(80)
    );

    let routes = config_client.list_routes();
    assert_eq!(routes.data.len(), 1);
    assert_eq!(routes.resource_version, 4);
    assert_eq!(routes.data[0].metadata.name.as_deref(), Some("demo-route"));
    assert_eq!(
        routes.data[0]
            .spec
            .hostnames
            .as_ref()
            .and_then(|hosts| hosts.first())
            .map(|s| s.as_str()),
        Some("example.com")
    );

    let services = config_client.list_services();
    assert_eq!(services.data.len(), 1);
    assert_eq!(services.resource_version, 5);
    assert_eq!(
        services.data[0].metadata.name.as_deref(),
        Some("demo-service")
    );

    let endpoint_slices = config_client.list_endpoint_slices();
    assert_eq!(endpoint_slices.data.len(), 1);
    assert_eq!(endpoint_slices.resource_version, 6);
    assert_eq!(
        endpoint_slices.data[0].metadata.name.as_deref(),
        Some("demo-slice")
    );

    let edgion_tls = config_client.list_edgion_tls();
    assert_eq!(edgion_tls.data.len(), 1);
    assert_eq!(edgion_tls.resource_version, 7);
    assert_eq!(
        edgion_tls.data[0].spec.secret_ref.name.as_deref(),
        Some("demo-secret")
    );

    let secrets = config_client.list_secrets();
    assert_eq!(secrets.data.len(), 1);
    assert_eq!(secrets.resource_version, 8);
    assert_eq!(
        secrets.data[0].metadata.name.as_deref(),
        Some("demo-secret")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn config_server_http_route_lifecycle_syncs() {
    let mut config_server = ConfigServer::new();
    let mut config_client = ConfigClient::new(GATEWAY_CLASS_KEY.to_string());

    exercise_http_route_lifecycle(&mut config_server, &mut config_client).await;
}

fn apply_watch_response_to_hub(hub: &mut ClientCache<HTTPRoute>, response: WatchResponse<HTTPRoute>) {
    let WatchResponse {
        events,
        err,
        resource_version: _,
    } = response;

    assert!(
        err.is_none(),
        "unexpected error in watch response: {:?}",
        err
    );

    for event in events {
        let change = match event.event_type {
            EventType::Add => ResourceChange::EventAdd,
            EventType::Update => ResourceChange::EventUpdate,
            EventType::Delete => ResourceChange::EventDelete,
        };
        hub.apply_change(change, event.data, Some(event.resource_version));
    }
}

async fn drain_watch_events(
    hub: &mut ClientCache<HTTPRoute>,
    receiver: &mut mpsc::Receiver<WatchResponse<HTTPRoute>>,
) {
    loop {
        match timeout(Duration::from_secs(2), receiver.recv()).await {
            Ok(Some(response)) => apply_watch_response_to_hub(hub, response),
            _ => break,
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cache_diff_confirms_hub_matches_center_after_events() {
    let mut center = ServerCache::<HTTPRoute>::new(32);
    center.set_ready();
    let mut hub = ClientCache::<HTTPRoute>::new();

    let initial_routes = vec![
        make_http_route("route-a", "a.example.com", 1),
        make_http_route("route-b", "b.example.com", 2),
        make_http_route("route-c", "c.example.com", 3),
    ];

    for route in &initial_routes {
        center.apply_change(
            ResourceChange::EventAdd,
            route.clone(),
            Some(route.get_version()),
        );
    }

    sleep(Duration::from_millis(50)).await;

    let mut watch_rx = center.watch("hub-client".to_string(), "hub-http-route".to_string(), 0);

    if let Some(response) = watch_rx.recv().await {
        apply_watch_response_to_hub(&mut hub, response);
    }

    let updated_route_b = make_http_route("route-b", "b-updated.example.com", 2);
    center.apply_change(ResourceChange::EventUpdate, updated_route_b, Some(2));

    sleep(Duration::from_millis(50)).await;

    if let Some(response) = watch_rx.recv().await {
        apply_watch_response_to_hub(&mut hub, response);
    }

    let new_route = make_http_route("route-d", "d.example.com", 4);
    center.apply_change(ResourceChange::EventAdd, new_route, Some(4));

    sleep(Duration::from_millis(50)).await;

    if let Some(response) = watch_rx.recv().await {
        apply_watch_response_to_hub(&mut hub, response);
    }

    let to_delete = make_http_route("route-a", "a.example.com", 1);
    center.apply_change(ResourceChange::EventDelete, to_delete, Some(1));

    sleep(Duration::from_millis(50)).await;

    if let Some(response) = watch_rx.recv().await {
        apply_watch_response_to_hub(&mut hub, response);
    }

    sleep(Duration::from_secs(2)).await;

    let diff = diff_center_hub(&center, &hub).await;
    assert!(
        diff.is_empty(),
        "expected hub to match center after watch events, diff: {:?}",
        diff
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cache_diff_verifies_full_sync_after_mixed_mutations() {
    let mut center = ServerCache::<HTTPRoute>::new(32);
    center.set_ready();
    let mut hub = ClientCache::<HTTPRoute>::new();

    let initial_routes = vec![
        make_http_route("route-alpha", "alpha.example.com", 10),
        make_http_route("route-bravo", "bravo.example.com", 20),
        make_http_route("route-charlie", "charlie.example.com", 30),
    ];

    for route in &initial_routes {
        center.apply_change(
            ResourceChange::EventAdd,
            route.clone(),
            Some(route.get_version()),
        );
    }

    sleep(Duration::from_millis(50)).await;

    let mut watch_rx = center.watch(
        "hub-client-mixed".to_string(),
        "hub-http-route-mixed".to_string(),
        0,
    );
    drain_watch_events(&mut hub, &mut watch_rx).await;

    let updated_bravo = make_http_route("route-bravo", "bravo-updated.example.com", 20);
    center.apply_change(ResourceChange::EventUpdate, updated_bravo, Some(20));

    sleep(Duration::from_millis(1)).await;
    drain_watch_events(&mut hub, &mut watch_rx).await;

    let new_route = make_http_route("route-delta", "delta.example.com", 40);
    center.apply_change(ResourceChange::EventAdd, new_route, Some(40));

    sleep(Duration::from_millis(50)).await;
    drain_watch_events(&mut hub, &mut watch_rx).await;

    let delete_route = make_http_route("route-alpha", "alpha.example.com", 10);
    center.apply_change(ResourceChange::EventDelete, delete_route, Some(10));

    sleep(Duration::from_millis(50)).await;
    drain_watch_events(&mut hub, &mut watch_rx).await;

    sleep(Duration::from_secs(2)).await;
    drain_watch_events(&mut hub, &mut watch_rx).await;

    let diff = diff_center_hub(&center, &hub).await;
    assert!(
        diff.is_empty(),
        "expected hub and center caches to match after mixed mutations, diff: {:?}",
        diff
    );
}
