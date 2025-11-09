#![cfg(test)]

use crate::core::conf_sync::config_server::ConfigServer;
use crate::core::conf_sync::grpc_client::ConfigSyncClient;
use crate::core::conf_sync::grpc_server::ConfigSyncServer;
use crate::core::conf_sync::traits::{EventDispatcher, ResourceChange};
use crate::types::{
    EdgionGatewayConfig, EdgionGatewayConfigSpec, EdgionTls, EdgionTlsSpec, ResourceKind,
};
use k8s_openapi::api::core::v1::SecretReference;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

const GATEWAY_CLASS_KEY: &str = "test-gateway-class";

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

async fn start_test_server(
    config_center: Arc<Mutex<ConfigServer>>,
) -> (
    std::net::SocketAddr,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("retrieve listener address");

    let incoming = TcpListenerStream::new(listener);
    let service = ConfigSyncServer::new_with_shared(config_center).into_service();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let handle = tokio::spawn(async move {
        let shutdown = async {
            let _ = shutdown_rx.await;
        };

        Server::builder()
            .add_service(service)
            .serve_with_incoming_shutdown(incoming, shutdown)
            .await
            .expect("serve grpc");
    });

    (addr, shutdown_tx, handle)
}

async fn seed_all_resource_types(config_center: &Arc<Mutex<ConfigServer>>) {
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

    let resources = vec![
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
            json!({
                "apiVersion": "gateway.networking.k8s.io/v1",
                "kind": "HTTPRoute",
                "metadata": {
                    "name": "demo-route",
                    "namespace": "default",
                    "resourceVersion": "4"
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
                    "hostnames": ["example.com"]
                }
            }),
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
    ];

    let mut center = config_center.lock().await;
    for (kind, value, version) in resources {
        let data = serde_json::to_string(&value).expect("serialize resource");
        <ConfigServer as EventDispatcher>::apply_resource_change(
            &mut *center,
            ResourceChange::EventAdd,
            Some(kind),
            data,
            Some(version),
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn grpc_server_client_syncs_all_resource_types() {
    let config_center = Arc::new(Mutex::new(ConfigServer::new()));

    let (addr, shutdown_tx, server_handle) = start_test_server(config_center.clone()).await;

    // Ensure the server is listening before proceeding.
    sleep(Duration::from_millis(50)).await;

    seed_all_resource_types(&config_center).await;

    let endpoint = format!("http://{}", addr);
    let mut client = ConfigSyncClient::connect(endpoint, GATEWAY_CLASS_KEY.to_string())
        .await
        .expect("connect client");

    client.sync_all().await.expect("sync all resources");

    let hub = client.get_config_hub();
    let hub_guard = hub.lock().await;

    assert_eq!(hub_guard.list_gateway_classes().data.len(), 1);
    assert_eq!(hub_guard.list_edgion_gateway_config().data.len(), 1);
    assert_eq!(hub_guard.list_gateways().data.len(), 1);
    assert_eq!(hub_guard.list_routes().data.len(), 1);
    assert_eq!(hub_guard.list_services().data.len(), 1);
    assert_eq!(hub_guard.list_endpoint_slices().data.len(), 1);
    assert_eq!(hub_guard.list_edgion_tls().data.len(), 1);
    assert_eq!(hub_guard.list_secrets().data.len(), 1);

    drop(hub_guard);

    // Shut down gRPC server gracefully
    let _ = shutdown_tx.send(());
    let _ = server_handle.await;
}
