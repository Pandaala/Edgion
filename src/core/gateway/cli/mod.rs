use crate::core::controller::conf_mgr::conf_center::EndpointMode;
use crate::core::gateway::api;
use crate::core::gateway::backends::init_global_endpoint_mode;
use crate::core::gateway::cli::pingora::{create_and_configure_server, run_server};
use crate::core::gateway::conf_sync::{ConfigClient, ConfigSyncClient};
use crate::core::gateway::lb::leastconn::BackendCleaner;
use crate::core::gateway::observe::access_log::init_access_logger;
use crate::core::gateway::observe::logs::{
    init_logging, init_ssl_logger, init_tcp_logger, init_tls_logger, init_udp_logger,
};
use crate::core::gateway::observe::metrics;
use crate::types::{init_work_dir, work_dir};
use anyhow::{anyhow, Result};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub mod config;
mod pingora;

use crate::core::gateway::conf_sync::init_global_config_client;
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
            config.conf_sync_security.tls.as_ref(),
        )
        .await
        .map_err(|e| anyhow!("Failed to create ConfigSyncClient: {}", e))?;

        Ok(sync_client)
    }

    /// Start auxiliary services
    async fn start_auxiliary_services(
        config_client: Arc<ConfigClient>,
        config: &EdgionGatewayConfig,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        // Start backend cleaner
        let cleaner = BackendCleaner::new();
        cleaner.start();
        tracing::info!("Backend cleaner task started for LeastConnection LB");

        // Initialize health check stores and manager (tasks start on resource updates).
        let _ = crate::core::gateway::backends::health::check::get_hc_config_store();
        let _ = crate::core::gateway::backends::health::check::get_health_check_manager();
        tracing::info!("Health check manager initialized");

        // Spawn Admin API server with graceful shutdown
        let config_client_for_admin = config_client.clone();
        let mut admin_shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            let shutdown_future = async move {
                while !*admin_shutdown.borrow() {
                    if admin_shutdown.changed().await.is_err() {
                        break;
                    }
                }
            };
            if let Err(e) = api::serve_with_shutdown(config_client_for_admin, 5900, shutdown_future).await {
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

        // Spawn Metrics API server with graceful shutdown
        let metrics_cfg = config.metrics.clone();
        let mut metrics_shutdown = shutdown_rx.clone();
        tokio::spawn(async move {
            let shutdown_future = async move {
                while !*metrics_shutdown.borrow() {
                    if metrics_shutdown.changed().await.is_err() {
                        break;
                    }
                }
            };
            if let Err(e) = metrics::serve_with_shutdown(&metrics_cfg, shutdown_future).await {
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
            listen = %config.metrics.listen,
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

        // 5. Initialize RateLimit global configuration
        config::init_rate_limit_global_config(&config.rate_limit);
        config::init_lru_cache_global_config(&config.lru_cache);
        config::init_plugin_metadata_config(config.plugin_metadata.clone());

        // 5.1 Initialize AllEndpointStatus global configuration
        crate::core::gateway::plugins::http::all_endpoint_status::plugin::init_all_endpoint_status_global_config(
            &config.all_endpoint_status,
        );

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

        // 4.1. Get server info from Controller (includes EndpointMode and supported kinds)
        let server_info = runtime
            .block_on(sync_client.get_server_info())
            .map_err(|e| anyhow!("Failed to get server info from Controller: {}", e))?;

        // 4.2. Parse and initialize global endpoint mode from Controller
        let endpoint_mode = match server_info.endpoint_mode.as_str() {
            "EndpointSlice" => EndpointMode::EndpointSlice,
            "Endpoint" => EndpointMode::Endpoint,
            "Both" => EndpointMode::Both,
            _ => {
                tracing::warn!(
                    component = "startup",
                    endpoint_mode = %server_info.endpoint_mode,
                    "Unknown endpoint mode from Controller, defaulting to EndpointSlice"
                );
                EndpointMode::EndpointSlice
            }
        };
        init_global_endpoint_mode(endpoint_mode);

        // 4.3. Initialize integration testing mode from CLI flag
        // This flag activates: Access Log Store + Metrics Test Data collection
        let integration_testing = self.config.integration_testing_mode;
        crate::core::common::config::init_integration_testing_mode(integration_testing);

        // 4.4. Initialize global test mode using the same flag
        crate::core::common::config::init_global_test_mode(integration_testing);

        tracing::info!(
            component = "startup",
            endpoint_mode = ?endpoint_mode,
            test_mode = integration_testing,
            integration_testing_mode = integration_testing,
            server_id = %server_info.server_id,
            "Global endpoint mode initialized from Controller"
        );

        // 5. Start watching resources based on Controller's supported kinds
        runtime
            .block_on(sync_client.start_watch_kinds(&server_info.supported_kinds))
            .map_err(|e| anyhow!("Failed to start watching resources: {}", e))?;
        tracing::info!(
            supported_kinds = ?server_info.supported_kinds,
            "Started watching resources from Controller"
        );

        // 5.1. Start watching server metadata (gateway instance count for Cluster scope rate limiting)
        let sync_client_arc = Arc::new(sync_client);
        let sync_client_for_meta = sync_client_arc.clone();
        runtime.spawn(async move {
            sync_client_for_meta.start_watch_server_meta().await;
        });

        // 5.2 Initialize metrics configuration (global OnceLock for hot-path access)
        config::init_request_duration_config(config.metrics.request_duration.clone());

        // 6. Start auxiliary services
        // Create shutdown channel for auxiliary services
        let (aux_shutdown_tx, aux_shutdown_rx) = tokio::sync::watch::channel(false);
        runtime.block_on(Self::start_auxiliary_services(
            config_client.clone(),
            &config,
            aux_shutdown_rx.clone(),
        ));

        // 7. Wait for all resources ready (including Gateway/GatewayClass/EdgionGatewayConfig)
        runtime.block_on(Self::wait_for_ready(config_client.clone()))?;

        // 8. Preload LBs for all routes (warm-up to reduce first-request latency)
        crate::core::gateway::backends::preload_load_balancers(config_client.clone());

        // 9. Initialize all loggers
        runtime.block_on(init_access_logger(&config.access_log))?;
        runtime.block_on(init_ssl_logger(&config.ssl_log))?;
        runtime.block_on(init_tcp_logger(&config.tcp_log))?;
        runtime.block_on(init_tls_logger(&config.tls_log))?;
        runtime.block_on(init_udp_logger(&config.udp_log))?;

        tracing::info!("All loggers initialized (access, ssl, tcp, tls, udp)");

        // 10. Create and configure Pingora server (in Tokio runtime context for UDP listeners)
        let pingora_server = runtime.block_on(async {
            tokio::task::spawn_blocking(move || create_and_configure_server(config_client, &config))
                .await
                .expect("Failed to spawn blocking task")
        })?;

        // 11. Move runtime to background thread with shutdown-aware lifecycle
        let bg_shutdown_rx = aux_shutdown_rx;
        let bg_thread = std::thread::spawn(move || {
            runtime.block_on(async {
                let mut rx = bg_shutdown_rx;
                while !*rx.borrow() {
                    if rx.changed().await.is_err() {
                        break;
                    }
                }
                // Grace period for auxiliary services to drain
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            });
        });

        // 12. Run Pingora server (blocks until shutdown)
        run_server(pingora_server);

        // 13. Signal auxiliary services to shut down
        tracing::info!(
            component = "gateway",
            "Pingora stopped, shutting down auxiliary services"
        );
        let _ = aux_shutdown_tx.send(true);

        // 14. Wait for background runtime to complete (services drain + 5s grace)
        let _ = bg_thread.join();

        tracing::info!(component = "gateway", "Gateway shutdown complete");
        Ok(())
    }
}
