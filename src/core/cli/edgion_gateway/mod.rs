use crate::core::conf_sync::conf_client::{ConfigClient, ConfigSyncClient};
use crate::core::gateway::gateway_base::GatewayBase;
use crate::core::observe::init_logging;
use crate::types::init_prefix_dir;
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

    /// Spawn a background task to periodically print all gateway class configs every 10 seconds
    /// This can be easily removed in the future if not needed
    fn spawn_config_printer(config_client: Arc<ConfigClient>) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(120));
            loop {
                interval.tick().await;
                config_client.print_config();
            }
        });
    }

    /// Async bootstrap function that handles all async initialization
    /// 
    /// Returns a tuple of (GatewayBase, WorkerGuard).
    /// The WorkerGuard MUST be kept alive for logging to work properly.
    async fn bootstrap(&self) -> Result<(Arc<GatewayBase>, tracing_appender::non_blocking::WorkerGuard)> {
        // Load and merge configuration
        let config = EdgionGatewayConfig::load(self.config.clone())?;

        // Initialize and create prefix directory
        init_prefix_dir(&config.prefix_dir)
            .map_err(|e| anyhow!("Failed to create prefix directory {:?}: {}", &config.prefix_dir, e))?;
        
        // Initialize logging system with configuration from config file
        let log_config = config.to_log_config();
        
        tracing::info!("Logging will be initialized at: {:?}", log_config.log_dir);
        
        // Log allocator information
        tracing::info!(
            component = "startup",
            allocator = crate::allocator_name(),
            version = env!("CARGO_PKG_VERSION"),
            "Starting Edgion Gateway"
        );
        
        // Initialize logging and get the WorkerGuard
        // The guard owns a background thread that performs actual file writes
        let log_guard = init_logging(log_config).await?;

        let server_addr_opt = config.server_addr();
        let server_addr = server_addr_opt
            .as_deref()
            .ok_or_else(|| anyhow!("server_addr is required, please provide --server-addr or set in config file"))?;

        let gateway_class = config
            .gateway_class()
            .ok_or_else(|| anyhow!("gateway_class is required, please provide --gateway-class or set in config file"))?;

        let mut sync_client = ConfigSyncClient::new(
            server_addr,
            gateway_class,
            "edgion-gateway".to_string(),
            Duration::from_secs(10),
        )
        .await?;

        // Get config_client and set as global immediately
        let config_client = sync_client.get_config_client();
        crate::core::conf_sync::init_global_config_client(config_client.clone())
            .map_err(|e| anyhow!("Failed to initialize global config client: {}", e))?;

        // Initialize base configuration and sync all resources
        sync_client.init_base_conf().await?;

        // Initialize GatewayBase and bootstrap (before start_watch_all)
        let base_conf = config_client.get_base_conf()
            .ok_or_else(|| anyhow!("Base configuration not available"))?;

        // bootstrap Gateway with access_log config
        let gateway = Arc::new(GatewayBase::new(base_conf, Some(&config.access_log)));
        gateway.bootstrap()?;
        tracing::info!("Gateway bootstrap completed successfully");

        // Start watching for changes
        sync_client.start_watch_all().await?;

        Self::spawn_config_printer(config_client.clone());

        // Spawn Admin API server in background
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

        // Spawn Metrics API server in background
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

        // Wait for all caches to be ready
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            match config_client.is_ready() {
                Ok(()) => {
                    tracing::info!("All caches are ready");
                    break;
                }
                Err(msg) => {
                    tracing::info!("{}", msg);
                }
            }
        }
        
        Ok((gateway, log_guard))
    }

    pub fn run(&self) -> Result<()> {
        // Create a Tokio runtime for async operations
        let runtime = tokio::runtime::Runtime::new()?;
        
        // Bootstrap all async operations in one block_on call
        // Keep the log_guard alive for the entire lifetime of the application
        let (gateway, _log_guard) = runtime.block_on(self.bootstrap())?;
        
        // Move the Tokio runtime to a background thread for async tasks
        // (config printer, config watchers, etc.)
        std::thread::spawn(move || {
            runtime.block_on(async {
                std::future::pending::<()>().await;
            });
        });
        
        // Run Pingora in the MAIN thread to properly handle system signals
        // This ensures graceful shutdown (Ctrl+C, SIGTERM) and zero-downtime upgrades work correctly
        // With tracing-appender, logging works from any thread without runtime context dependency
        gateway.run_forever();
        
        Ok(())
    }
}

