//! Pingora gateway runtime management

use crate::core::cli::edgion_gateway::config::EdgionGatewayConfig;
use crate::core::conf_sync::conf_client::ConfigClient;
use crate::core::gateway::gateway_base::GatewayBase;
use anyhow::Result;
use pingora_core::server::configuration::ServerConf;
use pingora_core::server::Server;
use std::sync::Arc;

/// Create Pingora ServerConf from local toml configuration
fn create_server_conf(config: &EdgionGatewayConfig) -> ServerConf {
    let mut conf = ServerConf::default();

    // Ensure daemon mode is disabled (we don't run as daemon)
    conf.daemon = false;

    // 1. Number of worker threads (default: number of CPU cores)
    conf.threads = config
        .server
        .threads
        .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1));

    // 2. Enable work stealing (default: true)
    conf.work_stealing = config.server.work_stealing.unwrap_or(true);

    // 3. Grace period for shutdown (default: 30 seconds)
    conf.grace_period_seconds = Some(config.server.grace_period_seconds.unwrap_or(30));

    // 4. Graceful shutdown timeout (default: 10 seconds)
    conf.graceful_shutdown_timeout_seconds = Some(config.server.graceful_shutdown_timeout_seconds.unwrap_or(10));

    // 5. Upstream keepalive pool size (default: 128)
    conf.upstream_keepalive_pool_size = config.server.upstream_keepalive_pool_size.unwrap_or(128);

    // 6. Error log file path (optional)
    conf.error_log = config.server.error_log.clone();

    tracing::debug!(
        threads = conf.threads,
        work_stealing = conf.work_stealing,
        grace_period = ?conf.grace_period_seconds,
        graceful_shutdown_timeout = ?conf.graceful_shutdown_timeout_seconds,
        upstream_keepalive_pool_size = conf.upstream_keepalive_pool_size,
        "Created Pingora ServerConf from local configuration"
    );

    conf
}

/// Phase 1: Create and configure Pingora server with listeners
///
/// This function:
/// 1. Creates Pingora ServerConf from local toml configuration
/// 2. Creates and bootstraps Pingora Server
/// 3. Fetches all Gateway resources from ConfigClient
/// 4. Delegates to GatewayBase to configure listeners for each Gateway
///
/// # Requirements
/// - AccessLogger must be initialized (via `init_access_logger()`)
/// - ConfigClient must be ready with Gateway/GatewayClass/EdgionGatewayConfig
pub fn create_and_configure_server(
    config_client: Arc<ConfigClient>,
    toml_config: &EdgionGatewayConfig,
) -> Result<Server> {
    // 1. Create ServerConf from local toml configuration
    let server_conf = create_server_conf(toml_config);

    // 2. Create and bootstrap Pingora Server
    let mut pingora_server = Server::new_with_opt_and_conf(None, server_conf);
    pingora_server.bootstrap();
    tracing::info!("Pingora server initialized");

    // 3. Create GatewayBase (only for Gateway logic, doesn't own pingora_server)
    let gateway_base = GatewayBase::new(config_client.clone());

    // 4. Fetch Gateway resources
    let gateways = config_client.list_gateways().data;

    // 5. Configure all listeners on the Pingora server
    gateway_base.configure_listeners(&mut pingora_server, gateways)?;
    tracing::info!("Gateway listeners configured");

    Ok(pingora_server)
}

/// Phase 2: Run Pingora server (synchronous, blocks until shutdown)
///
/// This function starts the Pingora server and blocks until shutdown.
/// It should be called after the Tokio runtime has been moved to a background thread.
pub fn run_server(mut server: Server) {
    tracing::info!("Starting Pingora server");
    server.run_forever();
}
