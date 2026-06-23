//! Pingora gateway runtime management

use crate::cli::config::EdgionGatewayConfig;
use crate::conf_sync::ConfigClient;
use crate::runtime::server::base::GatewayBase;
use anyhow::Result;
use pingora_core::server::configuration::ServerConf;
use pingora_core::server::Server;
use std::sync::Arc;

/// Create Pingora ServerConf from local YAML configuration
fn create_server_conf(config: &EdgionGatewayConfig) -> ServerConf {
    let threads = config
        .server
        .threads
        .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1));

    let work_stealing = config.server.work_stealing.unwrap_or(true);

    let grace_period_seconds = Some(config.server.grace_period_seconds.unwrap_or(30));

    let graceful_shutdown_timeout_seconds = Some(config.server.graceful_shutdown_timeout_seconds.unwrap_or(10));

    let upstream_keepalive_pool_size = config.server.upstream_keepalive_pool_size.unwrap_or(128);

    let error_log = config.server.error_log.clone();

    let conf = ServerConf {
        daemon: false,
        threads,
        work_stealing,
        grace_period_seconds,
        graceful_shutdown_timeout_seconds,
        upstream_keepalive_pool_size,
        error_log,
        ..Default::default()
    };

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

/// Create and configure the Pingora server with listeners.
///
/// 1. Builds `ServerConf` from local YAML configuration
/// 2. Creates and bootstraps the Pingora server
/// 3. Fetches all Gateway resources from `ConfigClient`
/// 4. Delegates to `GatewayBase` to configure listeners for each Gateway
///
/// # Requirements
/// - Access logger must be initialized (via `init_access_logger()`)
/// - `ConfigClient` must be ready with Gateway/GatewayClass/EdgionGatewayConfig
pub fn create_and_configure_server(
    config_client: Arc<ConfigClient>,
    gateway_config: &EdgionGatewayConfig,
) -> Result<Server> {
    // 1. Create ServerConf from local YAML configuration
    let server_conf = create_server_conf(gateway_config);

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

/// Run the Pingora server (synchronous, blocks until shutdown).
///
/// Call this after the Tokio runtime has been moved to a background thread.
pub fn run_server(server: Server) {
    tracing::info!("Starting Pingora server");
    server.run_forever();
}
