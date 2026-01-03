use crate::core::conf_sync::conf_client::{ConfigClient, ConfigSyncClient};
use crate::core::gateway::gateway_base::GatewayBase;
use crate::core::observe::init_logging;
use crate::types::prefix_dir;
use anyhow::{anyhow, Result};
use clap::Parser;
use std::sync::Arc;
use std::time::Duration;

pub mod config;

use config::EdgionGatewayConfig;

#[derive(Parser, Debug)]
#[command(
    name = "edgion-gateway",
    version,
    about = "Edgion Gateway standalone executable",
    long_about = None
)]
pub struct EdgionGatewayCli {
    #[command(flatten)]
    pub config: EdgionGatewayConfig,
}

impl EdgionGatewayCli {
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Create ConfigSyncClient
    async fn create_config_sync_client(config: &EdgionGatewayConfig) -> Result<ConfigSyncClient> {
        let server_addr_opt = config.server_addr();
        let server_addr = server_addr_opt
            .as_deref()
            .ok_or_else(|| anyhow!("server_addr is required, please provide --server-addr or set in config file"))?;
        
        let sync_client = ConfigSyncClient::new(
            server_addr,
            "edgion-gateway".to_string(),
            Duration::from_secs(10),
        ).await
        .map_err(|e| anyhow!("Failed to create ConfigSyncClient: {}", e))?;
        
        Ok(sync_client)
    }
    
    /// Start auxiliary services
    async fn start_auxiliary_services(config_client: Arc<ConfigClient>) {
        // Spawn config printer
        Self::spawn_config_printer(config_client.clone());
        
        // Start backend cleaner
        let cleaner = crate::core::lb::leastconn::BackendCleaner::new();
        cleaner.start();
        tracing::info!("Backend cleaner task started for LeastConnection LB");
        
        // Spawn Admin API server
        let config_client_for_admin = config_client.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::core::api::gateway::serve(config_client_for_admin, 5900).await {
                tracing::error!(
                    component = "admin_api_gateway",
                    event = "server_error",
                    error = %e,
                    "Gateway Admin API server failed"
                );
            }
        });
        
        tracing::info!(
            component = "admin_api_gateway",
            event = "server_spawned",
            port = 5900,
            "Gateway Admin API server spawned"
        );
        
        // Spawn Metrics API server
        tokio::spawn(async move {
            if let Err(e) = crate::core::api::metrics::serve(5901).await {
                tracing::error!(
                    component = "metrics_api",
                    event = "server_error",
                    error = %e,
                    "Metrics API server failed"
                );
            }
        });
        
        tracing::info!(
            component = "metrics_api",
            event = "server_spawned",
            port = 5901,
            "Metrics API server spawned"
        );
    }
    
    /// Wait for all caches to be ready
    async fn wait_for_ready(config_client: Arc<ConfigClient>) -> Result<()> {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            match config_client.is_ready() {
                Ok(()) => {
                    tracing::info!("All caches are ready");
                    return Ok(());
                }
                Err(msg) => {
                    tracing::info!("{}", msg);
                }
            }
        }
    }
    
    /// Spawn config printer task
    fn spawn_config_printer(config_client: Arc<ConfigClient>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(120));
            loop {
                interval.tick().await;
                config_client.print_config();
            }
        });
    }

    pub fn run(&self) -> Result<()> {
        // Create a Tokio runtime for async operations
        let runtime = tokio::runtime::Runtime::new()?;
        
        // 1. Load configuration (synchronous)
        let config = EdgionGatewayConfig::load(self.config.clone())?;
        let _ = prefix_dir();
        
        // 2. Initialize logging (at outermost level, keep WorkerGuard alive)
        let log_config = config.to_log_config();
        let _log_guard = runtime.block_on(init_logging(log_config))?;
        
        tracing::info!(
            component = "startup",
            allocator = crate::allocator_name(),
            version = env!("CARGO_PKG_VERSION"),
            "Starting Edgion Gateway"
        );
        
        // 3. Create ConfigSyncClient
        let mut sync_client = runtime.block_on(Self::create_config_sync_client(&config))?;
        let config_client = sync_client.get_config_client();
        
        // 4. Set global config client
        crate::core::conf_sync::init_global_config_client(config_client.clone())
            .map_err(|e| anyhow!("Failed to initialize global config client: {}", e))?;
        
        // 5. Start watching all resources
        runtime.block_on(sync_client.start_watch_all())?;
        tracing::info!("Started watching all resources");
        
        // 6. Start auxiliary services
        runtime.block_on(Self::start_auxiliary_services(config_client.clone()));
        
        // 7. Wait for all resources ready (including Gateway/GatewayClass/EdgionGatewayConfig)
        runtime.block_on(Self::wait_for_ready(config_client.clone()))?;
        
        // 8. Create AccessLogger
        let access_logger = runtime.block_on(
            crate::core::gateway::gateway_base::create_access_logger(&config.access_log)
        )?;
        
        // 9. Create GatewayBase
        let gateway = Arc::new(GatewayBase::new(config_client, access_logger));
        
        // 10. Bootstrap gateway (must be in Tokio runtime context for UDP listeners)
        runtime.block_on(async {
            gateway.bootstrap()
        })?;
        tracing::info!("Gateway bootstrap completed");
        
        // 11. Move runtime to background thread
        std::thread::spawn(move || {
            runtime.block_on(async {
                std::future::pending::<()>().await;
            });
        });
        
        // 12. Run Pingora in main thread (blocks until shutdown)
        gateway.run_forever();
        
        Ok(())
    }
}

