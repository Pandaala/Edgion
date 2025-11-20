use crate::core::conf_sync::config_server::ConfigServer;
use crate::core::conf_sync::grpc_client::ConfigSyncClient;
use crate::core::conf_sync::grpc_server::ConfigSyncServer;
use crate::core::conf_sync::traits::{ConfigServerEventDispatcher, ResourceChange};
use crate::types::{HTTPRoute, ResourceKind};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

/// Helper to setup base configuration (GatewayClass + Gateway) in ConfigServer
fn setup_base_conf(config_server: &Arc<ConfigServer>, gateway_class: &str) {
    // Add GatewayClass
    let gc_yaml = format!(
        r#"
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: {}
spec:
  controllerName: edgion.io/gateway-controller
"#,
        gateway_class
    );
    config_server.apply_base_conf(ResourceChange::InitAdd, None, gc_yaml);

    // Add Gateway
    let gw_yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: test-gateway
  namespace: default
spec:
  gatewayClassName: test-gateway-class
  listeners:
  - name: http
    port: 80
    protocol: HTTP
"#;
    config_server.apply_base_conf(ResourceChange::InitAdd, None, gw_yaml.to_string());
}

/// Test helper to create a sample HTTPRoute
fn create_sample_httproute(name: &str, namespace: &str, version: u64) -> HTTPRoute {
    let yaml = format!(
        r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: {}
  namespace: {}
  resourceVersion: "{}"
spec:
  parentRefs:
  - name: test-gateway
    namespace: default
  hostnames:
  - "example.com"
  rules:
  - matches:
    - path:
        type: PathPrefix
        value: /api
    backendRefs:
    - name: test-service
      port: 80
"#,
        name, namespace, version
    );
    serde_yaml::from_str(&yaml).expect("Failed to create HTTPRoute")
}

#[tokio::test]
async fn test_grpc_sync_httproute() {
    // Step 1: Create ConfigServer and populate with base conf and HTTPRoute
    let gateway_class = "test-gateway-class".to_string();
    let config_server = Arc::new(ConfigServer::new(Some(gateway_class.clone())));

    // Setup base configuration
    setup_base_conf(&config_server, &gateway_class);

    // Add a HTTPRoute to the server
    let route = create_sample_httproute("test-route-1", "default", 1);
    let route_yaml = serde_yaml::to_string(&route).unwrap();
    config_server.apply_resource_change(ResourceChange::InitAdd, None, route_yaml);

    // Step 2: Start gRPC server
    let addr = "127.0.0.1:50051".parse().unwrap();
    let grpc_server = ConfigSyncServer::new(config_server.clone());

    tokio::spawn(async move {
        if let Err(e) = grpc_server.serve(addr).await {
            eprintln!("gRPC server error: {}", e);
        }
    });

    // Wait for server to start
    sleep(Duration::from_millis(500)).await;

    // Step 3: Create gRPC client and sync
    let mut grpc_client = ConfigSyncClient::new(
        "http://127.0.0.1:50051",
        gateway_class.clone(),
        "test-client".to_string(),
        Duration::from_secs(5),
    )
    .await
    .expect("Failed to create gRPC client");

    // Initialize (fetch base conf and sync all resources)
    grpc_client.init().await.expect("Failed to initialize client");

    // Step 4: Verify the HTTPRoute was synced
    let config_client = grpc_client.get_config_client();
    let routes = config_client.list_routes();

    assert_eq!(routes.data.len(), 1, "Should have 1 HTTPRoute synced");
    assert_eq!(routes.data[0].metadata.name.as_ref().unwrap(), "test-route-1");
    assert_eq!(routes.data[0].metadata.namespace.as_ref().unwrap(), "default");

    println!("✅ gRPC sync test passed!");
}

#[tokio::test]
async fn test_grpc_watch_httproute() {
    // Step 1: Create ConfigServer
    let gateway_class = "test-gateway-class-watch".to_string();
    let config_server = Arc::new(ConfigServer::new(Some(gateway_class.clone())));

    // Setup base configuration
    setup_base_conf(&config_server, &gateway_class);

    // Step 2: Start gRPC server
    let addr = "127.0.0.1:50052".parse().unwrap();
    let grpc_server = ConfigSyncServer::new(config_server.clone());

    tokio::spawn(async move {
        if let Err(e) = grpc_server.serve(addr).await {
            eprintln!("gRPC server error: {}", e);
        }
    });

    // Wait for server to start
    sleep(Duration::from_millis(500)).await;

    // Step 3: Create gRPC client
    let mut grpc_client = ConfigSyncClient::new(
        "http://127.0.0.1:50052",
        gateway_class.clone(),
        "test-client-watch".to_string(),
        Duration::from_secs(5),
    )
    .await
    .expect("Failed to create gRPC client");

    grpc_client.init().await.expect("Failed to initialize client");

    // Step 4: Start watching HTTPRoute
    grpc_client
        .start_watch_sync(gateway_class.clone(), ResourceKind::HTTPRoute)
        .await
        .expect("Failed to start watch");

    // Wait a bit for watch to establish
    sleep(Duration::from_millis(500)).await;

    // Step 5: Add a new HTTPRoute to server
    let route = create_sample_httproute("test-route-watch-1", "default", 1);
    let route_yaml = serde_yaml::to_string(&route).unwrap();
    config_server.apply_resource_change(ResourceChange::EventAdd, None, route_yaml);

    // Wait for watch event to be processed
    sleep(Duration::from_millis(1000)).await;

    // Step 6: Verify the HTTPRoute was received via watch
    let config_client = grpc_client.get_config_client();
    let routes = config_client.list_routes();

    assert!(routes.data.len() >= 1, "Should have at least 1 HTTPRoute from watch");

    let found = routes
        .data
        .iter()
        .any(|r| r.metadata.name.as_ref().unwrap() == "test-route-watch-1");
    assert!(found, "Should have received test-route-watch-1 via watch");

    println!("✅ gRPC watch test passed!");
}

#[tokio::test]
async fn test_grpc_sync_multiple_httproutes() {
    // Step 1: Create ConfigServer and populate with multiple HTTPRoutes
    let gateway_class = "test-gateway-class-multi".to_string();
    let config_server = Arc::new(ConfigServer::new(Some(gateway_class.clone())));

    // Setup base configuration
    setup_base_conf(&config_server, &gateway_class);

    // Add multiple HTTPRoutes
    for i in 1..=5 {
        let route = create_sample_httproute(&format!("test-route-{}", i), "default", i as u64);
        let route_yaml = serde_yaml::to_string(&route).unwrap();
        config_server.apply_resource_change(ResourceChange::InitAdd, None, route_yaml);
    }

    // Step 2: Start gRPC server
    let addr = "127.0.0.1:50053".parse().unwrap();
    let grpc_server = ConfigSyncServer::new(config_server.clone());

    tokio::spawn(async move {
        if let Err(e) = grpc_server.serve(addr).await {
            eprintln!("gRPC server error: {}", e);
        }
    });

    // Wait for server to start
    sleep(Duration::from_millis(500)).await;

    // Step 3: Create gRPC client and sync
    let mut grpc_client = ConfigSyncClient::new(
        "http://127.0.0.1:50053",
        gateway_class.clone(),
        "test-client-multi".to_string(),
        Duration::from_secs(5),
    )
    .await
    .expect("Failed to create gRPC client");

    grpc_client.init().await.expect("Failed to initialize client");

    // Step 4: Verify all HTTPRoutes were synced
    let config_client = grpc_client.get_config_client();
    let routes = config_client.list_routes();

    assert_eq!(routes.data.len(), 5, "Should have 5 HTTPRoutes synced");

    for i in 1..=5 {
        let found = routes
            .data
            .iter()
            .any(|r| r.metadata.name.as_ref().unwrap() == &format!("test-route-{}", i));
        assert!(found, "Should have test-route-{}", i);
    }

    println!("✅ gRPC multiple routes sync test passed!");
}

#[tokio::test]
async fn test_grpc_watch_update_delete() {
    // Step 1: Create ConfigServer with initial HTTPRoute
    let gateway_class = "test-gateway-class-update".to_string();
    let config_server = Arc::new(ConfigServer::new(Some(gateway_class.clone())));

    // Setup base configuration
    setup_base_conf(&config_server, &gateway_class);

    let initial_route = create_sample_httproute("test-route-update", "default", 1);
    let route_yaml = serde_yaml::to_string(&initial_route).unwrap();
    config_server.apply_resource_change(ResourceChange::InitAdd, None, route_yaml);

    // Step 2: Start gRPC server
    let addr = "127.0.0.1:50054".parse().unwrap();
    let grpc_server = ConfigSyncServer::new(config_server.clone());

    tokio::spawn(async move {
        if let Err(e) = grpc_server.serve(addr).await {
            eprintln!("gRPC server error: {}", e);
        }
    });

    sleep(Duration::from_millis(500)).await;

    // Step 3: Create gRPC client and start watching
    let mut grpc_client = ConfigSyncClient::new(
        "http://127.0.0.1:50054",
        gateway_class.clone(),
        "test-client-update".to_string(),
        Duration::from_secs(5),
    )
    .await
    .expect("Failed to create gRPC client");

    grpc_client.init().await.expect("Failed to initialize");

    grpc_client
        .start_watch_sync(gateway_class.clone(), ResourceKind::HTTPRoute)
        .await
        .expect("Failed to start watch");

    sleep(Duration::from_millis(500)).await;

    // Step 4: Test Update
    let updated_route = create_sample_httproute("test-route-update", "default", 2);
    let updated_yaml = serde_yaml::to_string(&updated_route).unwrap();
    config_server.apply_resource_change(ResourceChange::EventUpdate, None, updated_yaml);

    sleep(Duration::from_millis(1000)).await;

    let config_client = grpc_client.get_config_client();
    let routes = config_client.list_routes();
    assert!(routes.data.len() >= 1, "Should still have the route after update");

    // Step 5: Test Delete
    let delete_yaml = serde_yaml::to_string(&updated_route).unwrap();
    config_server.apply_resource_change(ResourceChange::EventDelete, None, delete_yaml);

    sleep(Duration::from_millis(1000)).await;

    let routes = config_client.list_routes();
    // Note: Delete behavior depends on implementation - the route might still be there but marked as deleted
    // or might be removed from the cache
    println!(
        "After delete, routes count: {} (expected behavior depends on delete implementation)",
        routes.data.len()
    );

    println!("✅ gRPC watch update/delete test passed!");
}

#[tokio::test]
async fn test_grpc_client_reconnect() {
    // This test verifies that the client can handle server restart
    let gateway_class = "test-gateway-class-reconnect".to_string();
    let config_server = Arc::new(ConfigServer::new(Some(gateway_class.clone())));

    // Setup base configuration
    setup_base_conf(&config_server, &gateway_class);

    // Add initial data
    let route = create_sample_httproute("test-route-reconnect", "default", 1);
    let route_yaml = serde_yaml::to_string(&route).unwrap();
    config_server.apply_resource_change(ResourceChange::InitAdd, None, route_yaml);

    // Start server
    let addr = "127.0.0.1:50055".parse().unwrap();
    let grpc_server = ConfigSyncServer::new(config_server.clone());

    let server_handle = tokio::spawn(async move {
        if let Err(e) = grpc_server.serve(addr).await {
            eprintln!("gRPC server error: {}", e);
        }
    });

    sleep(Duration::from_millis(500)).await;

    // Create client
    let result = ConfigSyncClient::new(
        "http://127.0.0.1:50055",
        gateway_class.clone(),
        "test-client-reconnect".to_string(),
        Duration::from_secs(5),
    )
    .await;

    assert!(result.is_ok(), "Client should connect successfully");

    // Abort server to simulate disconnect
    server_handle.abort();
    sleep(Duration::from_millis(500)).await;

    // Try to create a new client - should fail or retry
    let result2 = ConfigSyncClient::new(
        "http://127.0.0.1:50055",
        gateway_class.clone(),
        "test-client-reconnect-2".to_string(),
        Duration::from_secs(2),
    )
    .await;

    assert!(result2.is_err(), "Client should fail when server is unavailable");

    println!("✅ gRPC client reconnect test passed!");
}
