//! Pingora gateway runtime management

use crate::cli::config::EdgionGatewayConfig;
use crate::conf_sync::ConfigClient;
use crate::runtime::server::base::GatewayBase;
use crate::runtime::server::downstream_connections::ConnectionCounter;
use crate::runtime::server::downstream_h2::DownstreamH2Policy;
use crate::runtime::server::proxy_protocol_policy::{
    get_global_proxy_protocol_policy_runtime, ProxyProtocolPolicyRuntime,
};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use edgion_resources::resources::edgion_gateway_config::MAX_RETRY_ATTEMPTS;
use pingora_core::server::configuration::ServerConf;
use pingora_core::server::{RunArgs, Server};
#[cfg(unix)]
use pingora_core::server::{ShutdownSignal, ShutdownSignalWatch, UnixShutdownSignalWatch};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

// One Gateway per process. Once termination begins, readiness cannot recover.
static TERMINATING: AtomicBool = AtomicBool::new(false);

pub(crate) fn is_terminating() -> bool {
    TERMINATING.load(Ordering::Relaxed)
}

/// Pingora counts the initial upstream request inside `ServerConf.max_retries`,
/// while Edgion's public `attempts` fields count retries after that request.
const PINGORA_MAX_UPSTREAM_ATTEMPTS: usize = MAX_RETRY_ATTEMPTS as usize + 1;

/// Create Pingora ServerConf from local YAML configuration
fn create_server_conf(config: &EdgionGatewayConfig) -> Result<ServerConf> {
    let offload = match (
        config.server.downstream_tls_offload_threadpools,
        config.server.downstream_tls_offload_thread_per_pool,
    ) {
        (None, None) => None,
        (Some(pools @ 1..=4), Some(threads @ 1..=4)) => Some((pools, threads)),
        (Some(_), Some(_)) => bail!("downstream TLS offload pools and threads per pool must each be 1..=4"),
        _ => bail!("downstream TLS offload pools and threads per pool must be configured together"),
    };
    let threads = config
        .server
        .threads
        .filter(|threads| *threads != 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1));

    let work_stealing = config.server.work_stealing.unwrap_or(true);

    let grace_period_seconds = Some(config.server.grace_period_seconds.unwrap_or(30));

    let graceful_shutdown_timeout_seconds = Some(config.server.graceful_shutdown_timeout_seconds.unwrap_or(10));

    let upstream_keepalive_pool_size = config.server.upstream_keepalive_pool_size.unwrap_or(128);

    let error_log = config.server.error_log.clone();

    // Backend `System` trust reads the same bundle as outbound clients. Pingora
    // loads the file later, while listener construction builds its upstream
    // connectors, and unwraps that load. Recheck it here: a file BoringSSL
    // cannot load becomes an error instead of a panic, and a file replaced since
    // step 4d is refused instead of giving backends different anchors.
    let system_trust = edgion_common::system_trust::system_trust();
    let ca_file = system_trust
        .bundle_file()
        .map(|path| {
            validate_backend_ca_file(path)?;
            let reloaded = edgion_common::system_trust::SystemTrust::load(path)?;
            anyhow::ensure!(
                reloaded.certificates() == system_trust.certificates(),
                "system CA file {} changed after startup loaded it; restart the Gateway",
                path.display()
            );
            Ok(path.to_string_lossy().into_owned())
        })
        .transpose()?;

    let conf = ServerConf {
        daemon: false,
        threads,
        work_stealing,
        grace_period_seconds,
        graceful_shutdown_timeout_seconds,
        upstream_keepalive_pool_size,
        error_log,
        max_retries: PINGORA_MAX_UPSTREAM_ATTEMPTS,
        downstream_tls_offload_threadpools: offload.map(|(pools, _)| pools),
        downstream_tls_offload_thread_per_pool: offload.map(|(_, threads)| threads),
        ca_file,
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

    Ok(conf)
}

/// Prove BoringSSL can load the System CA bundle before Pingora does.
/// Pingora unwraps `set_ca_file` while building upstream connectors, so a file
/// it cannot load must fail startup here instead of panicking later. rustls
/// skips anchors it cannot use, so its acceptance does not imply BoringSSL's.
pub(crate) fn validate_backend_ca_file(path: &std::path::Path) -> Result<()> {
    #[cfg(feature = "boringssl")]
    {
        use pingora_core::tls::ssl::{SslConnector, SslMethod};
        let mut builder = SslConnector::builder(SslMethod::tls())
            .map_err(|error| anyhow::anyhow!("failed to create a BoringSSL connector: {error}"))?;
        builder.set_ca_file(path).map_err(|error| {
            anyhow::anyhow!(
                "BoringSSL cannot load the system CA file {}: {error}. \
                     Install a valid PEM CA bundle (ca-certificates) at that path",
                path.display()
            )
        })?;
    }
    #[cfg(not(feature = "boringssl"))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/cli/pingora_test/server_conf_tests.rs"]
mod server_conf_tests;

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
    let downstream_h2_policy = DownstreamH2Policy::try_from(&gateway_config.server)?;
    // One process-wide counter for every Pingora listener; unset = unlimited.
    let downstream_connections = Arc::new(ConnectionCounter::new(gateway_config.server.max_downstream_connections));
    // 1. Create ServerConf from local YAML configuration
    let server_conf = create_server_conf(gateway_config)?;

    // 2. Create and bootstrap Pingora Server
    let mut pingora_server = Server::new_with_opt_and_conf(None, server_conf);
    pingora_server.bootstrap();
    tracing::info!("Pingora server initialized");

    // 3. Create GatewayBase (only for Gateway logic, doesn't own pingora_server)
    let gateway_base = GatewayBase::new(config_client.clone(), gateway_config.gateway.bind_ip);

    // 4. Fetch Gateway resources
    let gateways = config_client.list_gateways().data;

    // 5. Configure all listeners on the Pingora server
    gateway_base.configure_listeners(
        &mut pingora_server,
        gateways,
        &downstream_h2_policy,
        &downstream_connections,
    )?;
    tracing::info!("Gateway listeners configured");

    Ok(pingora_server)
}

/// Run the Pingora server (synchronous, blocks until shutdown).
///
/// Call this after the Tokio runtime has been moved to a background thread.
pub fn run_server(server: Server, readiness_drain: Duration) -> Result<()> {
    tracing::info!("Starting Pingora server");
    let policy_runtime = get_global_proxy_protocol_policy_runtime();
    #[cfg(unix)]
    let run_args = RunArgs {
        shutdown_signal: Box::new(PolicyShutdownSignalWatch::new(
            UnixShutdownSignalWatch,
            policy_runtime.clone(),
            readiness_drain,
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
    readiness_drain: Duration,
}

#[cfg(unix)]
impl<W> PolicyShutdownSignalWatch<W> {
    fn new(external: W, policy_runtime: Arc<ProxyProtocolPolicyRuntime>, readiness_drain: Duration) -> Self {
        Self {
            external,
            policy_runtime,
            readiness_drain,
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
        let signal = tokio::select! {
            biased;
            _ = wait_for_policy_fatal(&mut fatal_rx) => return ShutdownSignal::FastShutdown,
            signal = self.external.recv() => signal,
        };
        if !matches!(signal, ShutdownSignal::GracefulTerminate) {
            return signal;
        }

        TERMINATING.store(true, Ordering::Relaxed);
        if self.readiness_drain.is_zero() {
            return signal;
        }

        let delay = tokio::time::sleep(self.readiness_drain);
        tokio::pin!(delay);
        loop {
            tokio::select! {
                biased;
                _ = wait_for_policy_fatal(&mut fatal_rx) => return ShutdownSignal::FastShutdown,
                _ = &mut delay => return signal,
                next = self.external.recv() => {
                    if matches!(next, ShutdownSignal::FastShutdown) {
                        return next;
                    }
                }
            }
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
#[path = "../../tests/unit/cli/pingora_test.rs"]
mod tests;
