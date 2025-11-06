use rand::Rng;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio::time::sleep;

use edgion::core::conf_sync::config_center::ConfigCenter;
use edgion::core::conf_sync::config_hub::ConfigHub;
use edgion::core::conf_sync::grpc_client::ConfigSyncClient;
use edgion::core::conf_sync::grpc_server::ConfigSyncServer;
use edgion::core::conf_sync::traits::EventDispatcher;
use edgion::types::ResourceKind;

const GATEWAY_CLASS_KEY: &str = "test-gateway-class";
const SERVER_ADDR: &str = "http://127.0.0.1:50051";

/// Generate a random resource of the given kind
fn generate_random_resource(kind: ResourceKind, index: u64) -> String {
    match kind {
        ResourceKind::GatewayClass => json!({
            "metadata": {
                "name": format!("gateway-class-{}", index),
                "resourceVersion": index.to_string()
            },
            "spec": {}
        })
        .to_string(),
        ResourceKind::GatewayClassSpec => json!({
            "metadata": {
                "name": format!("gateway-class-spec-{}", index),
                "resourceVersion": index.to_string()
            },
            "spec": {
                "controllerName": format!("controller-{}", index)
            }
        })
        .to_string(),
        ResourceKind::Gateway => json!({
            "metadata": {
                "name": format!("gateway-{}", index),
                "namespace": "default",
                "resourceVersion": index.to_string()
            },
            "spec": {
                "gatewayClassName": GATEWAY_CLASS_KEY,
                "listeners": []
            }
        })
        .to_string(),
        ResourceKind::HTTPRoute => json!({
            "metadata": {
                "name": format!("httproute-{}", index),
                "namespace": "default",
                "resourceVersion": index.to_string()
            },
            "spec": {
                "parentRefs": [{
                    "name": GATEWAY_CLASS_KEY
                }],
                "rules": []
            }
        })
        .to_string(),
        ResourceKind::Service => json!({
            "metadata": {
                "name": format!("service-{}", index),
                "namespace": "default",
                "resourceVersion": index.to_string()
            },
            "spec": {
                "ports": []
            }
        })
        .to_string(),
        ResourceKind::EndpointSlice => json!({
            "metadata": {
                "name": format!("endpointslice-{}", index),
                "namespace": "default",
                "resourceVersion": index.to_string()
            },
            "endpoints": []
        })
        .to_string(),
        ResourceKind::EdgionTls => json!({
            "metadata": {
                "name": format!("edgion-tls-{}", index),
                "namespace": "default",
                "resourceVersion": index.to_string()
            },
            "spec": {}
        })
        .to_string(),
        ResourceKind::Secret => json!({
            "metadata": {
                "name": format!("secret-{}", index),
                "namespace": "default",
                "resourceVersion": index.to_string()
            },
            "data": {}
        })
        .to_string(),
    }
}

/// Randomly generate and apply resources to ConfigCenter
async fn generate_random_configs(config_center: Arc<Mutex<ConfigCenter>>, duration: Duration) {
    let start = Instant::now();
    let resource_kinds = vec![
        ResourceKind::GatewayClass,
        ResourceKind::GatewayClassSpec,
        ResourceKind::Gateway,
        ResourceKind::HTTPRoute,
        ResourceKind::Service,
        ResourceKind::EndpointSlice,
        ResourceKind::EdgionTls,
        ResourceKind::Secret,
    ];

    let resource_counters = Arc::new(Mutex::new(HashMap::<ResourceKind, u64>::new()));
    {
        let mut counters = resource_counters.lock().await;
        for kind in &resource_kinds {
            counters.insert(*kind, 0);
        }
    }

    println!(
        "[SERVER] Starting random config generation for {:?}...",
        duration
    );

    while start.elapsed() < duration {
        // Create a new RNG for each random operation to avoid Send issues
        let kind = {
            let mut rng = rand::thread_rng();
            resource_kinds[rng.gen_range(0..resource_kinds.len())]
        };

        let operation = {
            let mut rng = rand::thread_rng();
            rng.gen_range(0..3)
        };

        let mut center = config_center.lock().await;
        let mut counters = resource_counters.lock().await;
        let counter = counters.get_mut(&kind).unwrap();

        match operation {
            0 => {
                // Add
                *counter += 1;
                let data = generate_random_resource(kind, *counter);
                center.event_add(Some(kind), data, Some(*counter));
                println!("[SERVER] Added {:?} #{}", kind, *counter);
            }
            1 => {
                // Update (only if we have resources)
                if *counter > 0 {
                    let update_index = {
                        let mut rng = rand::thread_rng();
                        rng.gen_range(1..=*counter)
                    };
                    let data = generate_random_resource(kind, update_index);
                    center.event_update(Some(kind), data, Some(update_index + 1000));
                    println!("[SERVER] Updated {:?} #{}", kind, update_index);
                }
            }
            2 => {
                // Delete (only if we have resources)
                if *counter > 0 {
                    let delete_index = {
                        let mut rng = rand::thread_rng();
                        rng.gen_range(1..=*counter)
                    };
                    let data = generate_random_resource(kind, delete_index);
                    center.event_del(Some(kind), data, Some(delete_index + 2000));
                    *counter -= 1;
                    println!("[SERVER] Deleted {:?} #{}", kind, delete_index);
                }
            }
            _ => {}
        }
        drop(center);
        drop(counters);

        // Random delay between operations (10-100ms)
        let delay_ms = {
            let mut rng = rand::thread_rng();
            rng.gen_range(10..100)
        };
        sleep(Duration::from_millis(delay_ms)).await;
    }

    println!("[SERVER] Finished random config generation");
}

/// Compare ConfigCenter and ConfigHub configurations
fn compare_configs(center: &ConfigCenter, hub: &ConfigHub, key: &String) -> bool {
    println!("\n[COMPARE] Comparing configurations for key: {}", key);

    let mut all_match = true;

    // Compare GatewayClasses
    let center_gc = center.list_gateway_classes(key);
    let hub_gc = hub.list_gateway_classes();
    if center_gc.as_ref().map(|d| d.data.len()) != Some(hub_gc.data.len()) {
        println!(
            "[COMPARE] ❌ GatewayClasses count mismatch: center={:?}, hub={}",
            center_gc.as_ref().map(|d| d.data.len()),
            hub_gc.data.len()
        );
        all_match = false;
    } else {
        println!(
            "[COMPARE] ✅ GatewayClasses count match: {}",
            hub_gc.data.len()
        );
    }

    // Compare GatewayClassSpecs
    let center_specs = center.list_gateway_class_specs(key);
    let hub_specs = hub.list_gateway_class_specs();
    if center_specs.as_ref().map(|d| d.data.len()) != Some(hub_specs.data.len()) {
        println!(
            "[COMPARE] ❌ GatewayClassSpecs count mismatch: center={:?}, hub={}",
            center_specs.as_ref().map(|d| d.data.len()),
            hub_specs.data.len()
        );
        all_match = false;
    } else {
        println!(
            "[COMPARE] ✅ GatewayClassSpecs count match: {}",
            hub_specs.data.len()
        );
    }

    // Compare Gateways
    let center_gw = center.list_gateways(key);
    let hub_gw = hub.list_gateways();
    if center_gw.as_ref().map(|d| d.data.len()) != Some(hub_gw.data.len()) {
        println!(
            "[COMPARE] ❌ Gateways count mismatch: center={:?}, hub={}",
            center_gw.as_ref().map(|d| d.data.len()),
            hub_gw.data.len()
        );
        all_match = false;
    } else {
        println!("[COMPARE] ✅ Gateways count match: {}", hub_gw.data.len());
    }

    // Compare HTTPRoutes
    let center_routes = center.list_routes(key);
    let hub_routes = hub.list_routes();
    if center_routes.as_ref().map(|d| d.data.len()) != Some(hub_routes.data.len()) {
        println!(
            "[COMPARE] ❌ HTTPRoutes count mismatch: center={:?}, hub={}",
            center_routes.as_ref().map(|d| d.data.len()),
            hub_routes.data.len()
        );
        all_match = false;
    } else {
        println!(
            "[COMPARE] ✅ HTTPRoutes count match: {}",
            hub_routes.data.len()
        );
    }

    // Compare Services
    let center_svc = center.list_services(key);
    let hub_svc = hub.list_services();
    if center_svc.as_ref().map(|d| d.data.len()) != Some(hub_svc.data.len()) {
        println!(
            "[COMPARE] ❌ Services count mismatch: center={:?}, hub={}",
            center_svc.as_ref().map(|d| d.data.len()),
            hub_svc.data.len()
        );
        all_match = false;
    } else {
        println!("[COMPARE] ✅ Services count match: {}", hub_svc.data.len());
    }

    // Compare EndpointSlices
    let center_es = center.list_endpoint_slices(key);
    let hub_es = hub.list_endpoint_slices();
    if center_es.as_ref().map(|d| d.data.len()) != Some(hub_es.data.len()) {
        println!(
            "[COMPARE] ❌ EndpointSlices count mismatch: center={:?}, hub={}",
            center_es.as_ref().map(|d| d.data.len()),
            hub_es.data.len()
        );
        all_match = false;
    } else {
        println!(
            "[COMPARE] ✅ EndpointSlices count match: {}",
            hub_es.data.len()
        );
    }

    // Compare EdgionTls
    let center_tls = center.list_edgion_tls(key);
    let hub_tls = hub.list_edgion_tls();
    if center_tls.as_ref().map(|d| d.data.len()) != Some(hub_tls.data.len()) {
        println!(
            "[COMPARE] ❌ EdgionTls count mismatch: center={:?}, hub={}",
            center_tls.as_ref().map(|d| d.data.len()),
            hub_tls.data.len()
        );
        all_match = false;
    } else {
        println!("[COMPARE] ✅ EdgionTls count match: {}", hub_tls.data.len());
    }

    // Compare Secrets
    let center_secrets = center.list_secrets(key);
    let hub_secrets = hub.list_secrets();
    if center_secrets.as_ref().map(|d| d.data.len()) != Some(hub_secrets.data.len()) {
        println!(
            "[COMPARE] ❌ Secrets count mismatch: center={:?}, hub={}",
            center_secrets.as_ref().map(|d| d.data.len()),
            hub_secrets.data.len()
        );
        all_match = false;
    } else {
        println!(
            "[COMPARE] ✅ Secrets count match: {}",
            hub_secrets.data.len()
        );
    }

    if all_match {
        println!("[COMPARE] ✅ All configurations match!");
    } else {
        println!("[COMPARE] ❌ Configuration mismatch detected!");
        println!("\n[COMPARE] ConfigCenter state:");
        center.print_config(&key.to_string());
        println!("\n[COMPARE] ConfigHub state:");
        hub.print_config();
    }

    all_match
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Config Sync Integration Test ===");
    println!("This test will:");
    println!("1. Start gRPC server and client");
    println!("2. Generate random configs for 30 seconds");
    println!("3. Wait 15 seconds for sync");
    println!("4. Compare server and client configurations");
    println!();

    // Create ConfigCenter
    let config_center = Arc::new(Mutex::new(ConfigCenter::new()));

    // Initialize ConfigCenter with the gateway class key
    {
        let mut center = config_center.lock().await;
        center.set_ready();
    }

    // Start gRPC server with shared ConfigCenter
    let server_addr = "0.0.0.0:50051".parse()?;
    let server = ConfigSyncServer::new_with_shared(config_center.clone());

    let server_handle = tokio::spawn(async move {
        println!("[SERVER] Starting gRPC server on {}...", server_addr);
        if let Err(e) = server.serve(server_addr).await {
            eprintln!("[SERVER] Server error: {}", e);
        }
    });

    // Wait for server to start
    sleep(Duration::from_secs(2)).await;

    // Create and connect client
    println!("[CLIENT] Connecting to server...");
    let mut client =
        ConfigSyncClient::connect(SERVER_ADDR.to_string(), GATEWAY_CLASS_KEY.to_string()).await?;
    println!("[CLIENT] Connected successfully");

    // Initial sync
    println!("[CLIENT] Performing initial sync...");
    client.sync_all().await?;
    println!("[CLIENT] Initial sync completed");

    // Start watching all resources
    println!("[CLIENT] Starting watch for all resources...");
    client.start_watch_all().await?;
    println!("[CLIENT] Watch started");

    // Generate random configs for 30 seconds
    let generate_handle = tokio::spawn(generate_random_configs(
        config_center.clone(),
        Duration::from_secs(30),
    ));

    // Wait for generation to complete
    generate_handle.await?;

    // Wait 15 seconds for sync to complete
    println!("\n[TEST] Waiting 15 seconds for sync to complete...");
    sleep(Duration::from_secs(15)).await;

    // Compare configurations
    println!("\n[TEST] Comparing configurations...");
    let center = config_center.lock().await;
    let hub_arc = client.get_config_hub();
    let hub = hub_arc.lock().await;

    let matches = compare_configs(&center, &hub, &GATEWAY_CLASS_KEY.to_string());

    drop(center);
    drop(hub);

    if matches {
        println!("\n✅ TEST PASSED: Configurations match!");
        Ok(())
    } else {
        println!("\n❌ TEST FAILED: Configurations do not match!");
        Err("Configuration mismatch".into())
    }
}
