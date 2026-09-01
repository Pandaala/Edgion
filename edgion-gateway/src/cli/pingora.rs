//! Pingora gateway runtime management

use crate::cli::config::EdgionGatewayConfig;
use crate::conf_sync::ConfigClient;
use crate::runtime::server::base::GatewayBase;
use crate::runtime::server::proxy_protocol_policy::{
    get_global_proxy_protocol_policy_runtime, ProxyProtocolPolicyRuntime,
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use pingora_core::server::configuration::ServerConf;
use pingora_core::server::{RunArgs, Server};
#[cfg(unix)]
use pingora_core::server::{ShutdownSignal, ShutdownSignalWatch, UnixShutdownSignalWatch};
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
pub fn run_server(server: Server) -> Result<()> {
    tracing::info!("Starting Pingora server");
    let policy_runtime = get_global_proxy_protocol_policy_runtime();
    #[cfg(unix)]
    let run_args = RunArgs {
        shutdown_signal: Box::new(PolicyShutdownSignalWatch::new(
            UnixShutdownSignalWatch,
            policy_runtime.clone(),
        )),
    };
    #[cfg(not(unix))]
    let run_args = RunArgs::default();

    server.run(run_args);
    if let Some(diagnostic) = policy_runtime.fatal_diagnostic() {
        return Err(anyhow!(diagnostic));
    }
    Ok(())
}

#[cfg(unix)]
struct PolicyShutdownSignalWatch<W> {
    external: W,
    policy_runtime: Arc<ProxyProtocolPolicyRuntime>,
}

#[cfg(unix)]
impl<W> PolicyShutdownSignalWatch<W> {
    fn new(external: W, policy_runtime: Arc<ProxyProtocolPolicyRuntime>) -> Self {
        Self {
            external,
            policy_runtime,
        }
    }
}

#[cfg(unix)]
#[async_trait]
impl<W> ShutdownSignalWatch for PolicyShutdownSignalWatch<W>
where
    W: ShutdownSignalWatch + Send + Sync,
{
    async fn recv(&self) -> ShutdownSignal {
        let mut fatal_rx = self.policy_runtime.subscribe_fatal();
        if *fatal_rx.borrow() {
            return ShutdownSignal::FastShutdown;
        }
        tokio::select! {
            signal = self.external.recv() => signal,
            _ = wait_for_policy_fatal(&mut fatal_rx) => ShutdownSignal::FastShutdown,
        }
    }
}

#[cfg(unix)]
async fn wait_for_policy_fatal(fatal_rx: &mut tokio::sync::watch::Receiver<bool>) {
    while !*fatal_rx.borrow() {
        if fatal_rx.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::future::pending;

    use edgion_resources::resources::gateway::{GatewaySpec, Listener};
    use edgion_resources::resources::proxy_protocol::{
        CanonicalProxyProtocolTrustPolicy, ProxyProtocolPolicySourceKind, ResolvedInboundProxyProtocol,
    };

    use super::*;

    #[derive(Debug)]
    struct ExternalFast;

    #[async_trait]
    impl ShutdownSignalWatch for ExternalFast {
        async fn recv(&self) -> ShutdownSignal {
            ShutdownSignal::FastShutdown
        }
    }

    #[derive(Debug)]
    struct ExternalPending;

    #[async_trait]
    impl ShutdownSignalWatch for ExternalPending {
        async fn recv(&self) -> ShutdownSignal {
            pending().await
        }
    }

    fn gateway(state: ResolvedInboundProxyProtocol) -> edgion_resources::Gateway {
        let mut gateway = edgion_resources::Gateway::new(
            "shutdown",
            GatewaySpec {
                gateway_class_name: "edgion".to_string(),
                listeners: Some(vec![Listener {
                    name: "tcp".to_string(),
                    hostname: None,
                    port: 19090,
                    protocol: "TCP".to_string(),
                    tls: None,
                    allowed_routes: None,
                }]),
                addresses: None,
                tls: None,
                resolved_inbound_proxy_protocol: state,
            },
        );
        gateway.metadata.namespace = Some("default".to_string());
        gateway
    }

    #[tokio::test]
    async fn external_shutdown_semantics_are_preserved() {
        let watcher = PolicyShutdownSignalWatch::new(ExternalFast, Arc::new(ProxyProtocolPolicyRuntime::new()));
        assert!(matches!(watcher.recv().await, ShutdownSignal::FastShutdown));
    }

    #[tokio::test]
    async fn internal_policy_fatal_requests_fast_shutdown() {
        let runtime = Arc::new(ProxyProtocolPolicyRuntime::new());
        let initial = gateway(ResolvedInboundProxyProtocol::Disabled);
        runtime.observe_snapshot([&initial]);
        runtime.preflight_and_freeze([&initial]).unwrap();

        let changed = gateway(ResolvedInboundProxyProtocol::Ready {
            listener_names: vec!["tcp".to_string()],
            policy: CanonicalProxyProtocolTrustPolicy::Required {},
            source: ProxyProtocolPolicySourceKind::Inline,
        });
        runtime.observe_snapshot([&changed]);

        let watcher = PolicyShutdownSignalWatch::new(ExternalPending, runtime);
        assert!(matches!(watcher.recv().await, ShutdownSignal::FastShutdown));
    }
}
