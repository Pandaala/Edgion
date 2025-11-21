use anyhow::Result;
use edgion::core::conf_load::{ConfigLoader, LocalPathLoader};
use edgion::core::conf_sync::{ConfigServerEventDispatcher, traits::ResourceChange};
use edgion::types::ResourceKind;
use std::path::PathBuf;
use std::sync::Arc;

// Mock dispatcher for testing (we don't actually use it)
struct MockDispatcher;

impl ConfigServerEventDispatcher for MockDispatcher {
    fn apply_resource_change(
        &self,
        _change: ResourceChange,
        _resource_type: Option<ResourceKind>,
        _data: String,
    ) {
        // No-op for testing
    }

    fn apply_base_conf(
        &self,
        _change: ResourceChange,
        _resource_type: Option<ResourceKind>,
        _data: String,
    ) {
        // No-op for testing
    }

    fn set_ready(&self) {
        // No-op for testing
    }

    fn enable_version_fix_mode(&self) {
        // No-op for testing
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("=== Testing load_base with examples config ===\n");

    // Get the config examples directory path
    let config_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("config")
        .join("examples");

    println!("Loading configuration from: {:?}\n", config_path);

    // Create LocalPathLoader with a mock dispatcher (required for construction)
    let dispatcher: Arc<dyn ConfigServerEventDispatcher> = Arc::new(MockDispatcher);
    let loader = LocalPathLoader::new(config_path, dispatcher);

    // Connect to the config source
    println!("Connecting to configuration source...");
    loader.connect().await?;
    println!("✓ Connected successfully\n");

    // Call load_base to load the base configuration
    println!("Loading base configuration...");
    match loader.load_base().await {
        Ok(base_conf) => {
            println!("✓ Base configuration loaded successfully!\n");
            
            // Display loaded configuration
            println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
            println!("📋 Loaded Configuration Summary");
            println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
            
            // GatewayClass
            let gc = base_conf.gateway_class();
            println!("🏷️  GatewayClass:");
            println!("   Name: {:?}", gc.metadata.name);
            println!("   Controller: {}", gc.spec.controller_name);
            if let Some(desc) = &gc.spec.description {
                println!("   Description: {}", desc);
            }
            println!();
            
            // EdgionGatewayConfig
            let egwc = base_conf.edgion_gateway_config();
            println!("⚙️  EdgionGatewayConfig:");
            println!("   Name: {:?}", egwc.metadata.name);
            println!("   Spec fields:");
            println!("   - Listener Defaults: {:?}", egwc.spec.listener_defaults.is_some());
            println!("   - Load Balancing: {:?}", egwc.spec.load_balancing.is_some());
            println!("   - Access Log: {:?}", egwc.spec.access_log.is_some());
            println!("   - Security: {:?}", egwc.spec.security.is_some());
            println!("   - Limits: {:?}", egwc.spec.limits.is_some());
            println!("   - Observability: {:?}", egwc.spec.observability.is_some());
            println!();
            
            // Gateways
            let gateways = base_conf.gateways();
            println!("🚪 Gateways ({})", gateways.len());
            for (i, gw) in gateways.iter().enumerate() {
                println!("   [{}] Name: {:?}", i + 1, gw.metadata.name);
                println!("       Namespace: {:?}", gw.metadata.namespace);
                println!("       GatewayClassName: {}", gw.spec.gateway_class_name);
                if let Some(listeners) = &gw.spec.listeners {
                    println!("       Listeners: {}", listeners.len());
                    for listener in listeners {
                        println!("         - {}: {} on port {}", 
                            listener.name, 
                            listener.protocol, 
                            listener.port
                        );
                    }
                } else {
                    println!("       Listeners: 0");
                }
                println!();
            }
            
            println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
            println!("✅ Test completed successfully!");
            
            Ok(())
        }
        Err(e) => {
            println!("❌ Failed to load base configuration: {}", e);
            Err(e)
        }
    }
}

