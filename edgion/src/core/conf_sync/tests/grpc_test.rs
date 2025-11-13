use std::sync::Arc;
use std::time::Duration;

use serde_json;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

use crate::core::conf_sync::cache_server::ServerCache;
use crate::core::conf_sync::traits::{EventDispatcher, ResourceChange};
use crate::core::conf_sync::{ConfigServer, ConfigSyncClient, ConfigSyncServer, EventDispatch};
use crate::types::{GatewayClass, GatewayClassSpec, ResourceKind};

fn sample_gateway_class(name: &str, version: u64) -> GatewayClass {
    let mut gc = GatewayClass::new(
        name,
        GatewayClassSpec {
            controller_name: "edgion.dev/controller".to_string(),
            description: None,
            parameters_ref: None,
        },
    );
    gc.metadata.resource_version = Some(version.to_string());
    gc
}

#[tokio::test(flavor = "multi_thread")]
async fn grpc_client_receives_watch_updates() {
    let key = "grpc-gateway-class".to_string();

    // Step 1: prepare a shared server cache with ready state
    let shared_server = Arc::new(Mutex::new(ConfigServer::new()));
    {
        let mut server = shared_server.lock().await;
        server
            .gateway_classes
            .write().unwrap()
            .insert(key.clone(), ServerCache::new(32));
        server
            .gateway_classes
            .write().unwrap()
            .get_mut(&key)
            .expect("cache exists")
            .set_ready();
    }

    // Step 2: start an in-process gRPC server bound to a random local address
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind listener");
    let addr = listener.local_addr().expect("listener address");
    let incoming = TcpListenerStream::new(listener);

    let grpc_service = ConfigSyncServer::new_with_shared(shared_server.clone()).into_service();

    let server_task = tokio::spawn(async move {
        Server::builder()
            .add_service(grpc_service)
            .serve_with_incoming(incoming)
            .await
            .expect("gRPC server terminated unexpectedly");
    });

    // Step 3: connect a client, perform initial sync and start watching
    let endpoint = format!("http://{}", addr);
    let mut client = ConfigSyncClient::connect(endpoint, key.clone())
        .await
        .expect("client connect");

    client
        .sync_resource(key.clone(), ResourceKind::GatewayClass)
        .await
        .expect("initial sync");
    client
        .start_watch_sync(key.clone(), ResourceKind::GatewayClass)
        .await
        .expect("start watch");

    // Step 4: push an event through the server pipeline
    {
        let mut server = shared_server.lock().await;
        let gc = sample_gateway_class(&key, 1);
        let payload = serde_json::to_string(&gc).expect("serialize gateway class");
        server.apply_resource_change(
            ResourceChange::EventAdd,
            Some(ResourceKind::GatewayClass),
            payload,
            Some(1),
        );
    }

    sleep(Duration::from_millis(200)).await;

    // Step 5: compare server snapshot with client hub state
    let server_snapshot = shared_server
        .lock()
        .await
        .list_gateway_classes(&key)
        .expect("server snapshot");
    let config_client_arc = client.get_config_client();
    let client_guard = config_client_arc.lock().await;
    let client_snapshot = client_guard.list_gateway_classes();

    assert_eq!(
        server_snapshot.resource_version,
        client_snapshot.resource_version
    );

    let server_names: Vec<_> = server_snapshot
        .data
        .iter()
        .filter_map(|gc| gc.metadata.name.clone())
        .collect();
    let client_names: Vec<_> = client_snapshot
        .data
        .iter()
        .filter_map(|gc| gc.metadata.name.clone())
        .collect();

    assert_eq!(server_names, client_names);

    server_task.abort();
}
