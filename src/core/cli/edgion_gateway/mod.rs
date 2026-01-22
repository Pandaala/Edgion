use crate::core::backends::init_global_endpoint_mode;
use crate::core::conf_mgr::conf_center::EndpointMode;
use crate::core::conf_sync::conf_client::{ConfigClient, ConfigSyncClient};
use crate::core::observe::access_log::init_access_logger;
use crate::core::observe::init_logging;
use crate::core::observe::ssl_log::init_ssl_logger;
use crate::core::observe::tcp_log::init_tcp_logger;
use crate::core::observe::udp_log::init_udp_logger;
use crate::types::{init_work_dir, work_dir};
use anyhow::{anyhow, Result};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub mod config;
mod pingora;

use crate::core::api::gateway;
use crate::core::cli::edgion_gateway::pingora::{create_and_configure_server, run_server};
use crate::core::conf_sync::init_global_config_client;
use crate::core::lb::leastconn::BackendCleaner;
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

        let sync_client = ConfigSyncClient::new(server_addr, "edgion-gateway".to_string(), Duration::from_secs(10))
            .await
            .map_err(|e| anyhow!("Failed to create ConfigSyncClient: {}", e))?;

        Ok(sync_client)
    }

    /// Start auxiliary services
    async fn start_auxiliary_services(config_client: Arc<ConfigClient>) {
        // Start backend cleaner
        let cleaner = BackendCleaner::new();
        cleaner.start();
        tracing::info!("Backend cleaner task started for LeastConnection LB");

        // Spawn Admin API server
        let config_client_for_admin = config_client.clone();
        tokio::spawn(async move {
            if let Err(e) = gateway::serve(config_client_for_admin, 5900).await {
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

    pub fn run(&self) -> Result<()> {
        // Create a Tokio runtime for async operations
        let runtime = tokio::runtime::Runtime::new()?;

        // 1. Load configuration (synchronous)
        let config = EdgionGatewayConfig::load(self.config.clone())?;

        // 2. Determine work_dir (priority: CLI > ENV > Config > Default)
        let work_dir_path = self
            .config
            .work_dir
            .clone()
            .or_else(|| std::env::var("EDGION_WORK_DIR").ok().map(PathBuf::from))
            .or_else(|| config.work_dir.clone())
            .unwrap_or_else(|| PathBuf::from("."));

        // 3. Initialize and validate work_dir
        init_work_dir(work_dir_path).map_err(|e| anyhow!("Failed to initialize work directory: {}", e))?;
        let wd = work_dir();
        wd.validate()
            .map_err(|e| anyhow!("Work directory validation failed: {}", e))?;

        tracing::info!(
            work_dir = %wd.base().display(),
            "Work directory initialized"
        );

        // 4. Initialize logging (at outermost level, keep WorkerGuard alive)
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
        init_global_config_client(config_client.clone())
            .map_err(|e| anyhow!("Failed to initialize global config client: {}", e))?;

        // 4.1. Initialize global endpoint mode
        // For Gateway, we use EndpointSlice as default (modern K8s clusters)
        // This can be made configurable in the future if needed
        init_global_endpoint_mode(EndpointMode::EndpointSlice);
        tracing::info!(
            component = "startup",
            endpoint_mode = "EndpointSlice",
            "Global endpoint mode initialized"
        );

        // 5. Start watching all resources
        runtime.block_on(sync_client.start_watch_all())?;
        tracing::info!("Started watching all resources");

        // 6. Start auxiliary services
        runtime.block_on(Self::start_auxiliary_services(config_client.clone()));

        // 7. Wait for all resources ready (including Gateway/GatewayClass/EdgionGatewayConfig)
        runtime.block_on(Self::wait_for_ready(config_client.clone()))?;

        // 8. Initialize all loggers
        runtime.block_on(init_access_logger(&config.access_log))?;
        runtime.block_on(init_ssl_logger(&config.ssl_log))?;
        runtime.block_on(init_tcp_logger(&config.tcp_log))?;
        runtime.block_on(init_udp_logger(&config.udp_log))?;

        tracing::info!("All loggers initialized (access, ssl, tcp, udp)");

        // 9. Create and configure Pingora server (in Tokio runtime context for UDP listeners)
        let pingora_server = runtime.block_on(async {
            tokio::task::spawn_blocking(move || create_and_configure_server(config_client, &config))
                .await
                .expect("Failed to spawn blocking task")
        })?;

        // 10. Move runtime to background thread to keep async tasks running
        std::thread::spawn(move || {
            runtime.block_on(async {
                std::future::pending::<()>().await;
            });
        });

        // 11. Run Pingora server (blocks until shutdown)
        run_server(pingora_server);

        Ok(())
    }
}
