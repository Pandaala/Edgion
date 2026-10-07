//! EtcdLinkClient — runtime Etcd client wrapper.
//!
//! Built from LinkSys CRD config. Gateway LinkSys owns construct / init / retire
//! through the persistent runner; Controller conf-center still calls
//! an explicit trusted bootstrap entry + `init` directly.
//! Wraps `etcd_client::Client` with:
//! - Upstream SDK connection recovery plus background health monitoring
//! - Health tracking via AtomicBool
//! - Namespace prefix support for key isolation
//!
//! Unlike fred (Redis), etcd-client uses gRPC multiplexing — a single connection
//! handles multiple concurrent streams, so no connection pool is needed.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use rand::Rng;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

use crate::link_sys_core::outbound_tls::{outbound_global_default, PreparedLinkSysTls};
use crate::link_sys_core::{HealthTransitionReporter, PublishedHealth};
use edgion_resources::credentials::materials::{BasicAuthMaterial, TlsMaterial};
use edgion_resources::resources::link_sys::etcd::{EtcdKeepAlive, EtcdTimeout};
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};

use super::config_mapping::{build_bootstrap_options, build_owned_options, health_probe_timeout, PreparedOptions};

/// Trusted Controller connection fields and direct credentials/PEM. This input
/// contains no resource reference or fabricated source authority.
#[derive(Debug)]
pub struct EtcdBootstrapConfig {
    pub endpoints: Vec<String>,
    pub auth: Option<BasicAuthMaterial>,
    pub tls: Option<TlsMaterial>,
    pub timeout: Option<EtcdTimeout>,
    pub keep_alive: Option<EtcdKeepAlive>,
    pub namespace: Option<String>,
}

struct EtcdConnectionInput {
    // The owning generation remains alive across reconnect/probe/election.
    _owner: Option<Arc<PreparedLinkSysTls>>,
    endpoints: Vec<String>,
    options: PreparedOptions,
    probe_timeout: Duration,
}

/// Runtime Etcd client wrapper.
///
/// One published generation for Gateway LinkSys, or the Controller conf-center
/// client when constructed without apply metadata.
///
/// Resource construction retains the actual owner and does not connect:
/// ```no_run
/// use std::sync::Arc;
/// use edgion_common::link_sys_core::providers::etcd::EtcdLinkClient;
/// let owner = serde_json::from_value(serde_json::json!({
///     "metadata":{"name":"etcd","namespace":"apps"},
///     "spec":{"type":"etcd","config":{"endpoints":["http://etcd.example:2379"]}}
/// })).unwrap();
/// let client = EtcdLinkClient::from_link_sys(Arc::new(owner)).unwrap();
/// ```
/// Detached configuration/material construction is unavailable:
/// ```compile_fail
/// use edgion_common::link_sys_core::{credential::CredentialState, providers::etcd::EtcdLinkClient};
/// use edgion_resources::resources::link_sys::etcd::EtcdClientConfig;
/// let config: EtcdClientConfig = serde_json::from_value(serde_json::json!({"endpoints":["http://etcd.example:2379"]})).unwrap();
/// let client = EtcdLinkClient::from_config("apps/etcd", &config, &CredentialState::NotRequired);
/// ```
pub struct EtcdLinkClient {
    /// etcd client (wrapped in RwLock for reconnect swaps)
    client: Arc<RwLock<Option<etcd_client::Client>>>,
    /// Immutable options and source retained across every connection path.
    connection: Arc<EtcdConnectionInput>,
    /// Human-readable name ("namespace/name")
    name: String,
    /// Atomic health flag
    healthy: Arc<PublishedHealth>,
    /// Namespace prefix (if configured)
    namespace: Option<String>,
    /// Shutdown signal sender
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// Handle to the spawned health-monitor task (populated by `init()`),
    /// aborted in `shutdown()` to request cancellation without joining it.
    monitor_handle: parking_lot::Mutex<Option<JoinHandle<()>>>,
    /// Apply fingerprint hashes captured by the Gateway runner (D19).
    fp_rebuild: [u8; 32],
    fp_secret: [u8; 32],
    fp_rv: Option<String>,
    /// Set before handover retire so the monitor stops mutating shared health.
    retired: Arc<parking_lot::Mutex<bool>>,
    shutdown_started: AtomicBool,
    /// How many configured endpoints recently accepted a bounded probe.
    endpoint_live: Arc<AtomicUsize>,
    /// Gateway runner only: sample per-endpoint reachability. Controller
    /// construction leaves this false so conf-center init is unchanged (D12).
    sample_endpoint_coverage: bool,
}

impl std::fmt::Debug for EtcdLinkClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EtcdLinkClient")
            .field("name", &self.name)
            .field("endpoints", &self.connection.endpoints)
            .field("healthy", &self.healthy.load())
            .field("namespace", &self.namespace)
            .finish()
    }
}

impl EtcdLinkClient {
    /// Create from an actual owner. Does not connect; call init next.
    pub fn from_link_sys(owner: Arc<LinkSys>) -> Result<Self> {
        Self::from_prepared_link_sys(Arc::new(PreparedLinkSysTls::from_link_sys(owner)?))
    }

    /// Consume a retained actual-owner selection and its captured defaults.
    pub fn from_prepared_link_sys(input: Arc<PreparedLinkSysTls>) -> Result<Self> {
        let SystemConfig::Etcd(config) = &input.owner().spec.config else {
            anyhow::bail!("Etcd construction requires an actual Etcd LinkSys");
        };
        let options = build_owned_options(&input)?;
        let name = input.owner().key_name_str();
        let namespace = config.namespace.clone();
        let endpoints = config.endpoints.clone();
        let probe_timeout = health_probe_timeout(config);
        Ok(Self::from_connection(
            name,
            namespace,
            EtcdConnectionInput {
                _owner: Some(input),
                endpoints,
                options,
                probe_timeout,
            },
        ))
    }

    /// Trusted startup configuration is separate from resource-reference authority.
    pub fn from_bootstrap(name: &str, config: EtcdBootstrapConfig) -> Result<Self> {
        let defaults = config.tls.as_ref().map(|_| outbound_global_default());
        let options = build_bootstrap_options(&config, defaults.as_deref())?;
        let probe_timeout = super::config_mapping::probe_timeout_for(config.timeout.as_ref());
        Ok(Self::from_connection(
            name.into(),
            config.namespace,
            EtcdConnectionInput {
                _owner: None,
                endpoints: config.endpoints,
                options,
                probe_timeout,
            },
        ))
    }

    fn from_connection(name: String, namespace: Option<String>, connection: EtcdConnectionInput) -> Self {
        let (shutdown_tx, _) = tokio::sync::watch::channel(false);

        Self {
            client: Arc::new(RwLock::new(None)),
            connection: Arc::new(connection),
            name,
            healthy: Arc::new(PublishedHealth::new(false)),
            namespace,
            shutdown_tx,
            monitor_handle: parking_lot::Mutex::new(None),
            fp_rebuild: [0; 32],
            fp_secret: [0; 32],
            fp_rv: None,
            retired: Arc::new(parking_lot::Mutex::new(false)),
            shutdown_started: AtomicBool::new(false),
            endpoint_live: Arc::new(AtomicUsize::new(0)),
            sample_endpoint_coverage: false,
        }
    }

    /// Attach Gateway apply fingerprint and enable per-endpoint coverage.
    /// Controller construction must not call this (D12).
    pub fn with_gateway_apply_meta(
        mut self,
        rebuild: [u8; 32],
        secret: [u8; 32],
        resource_version: Option<String>,
    ) -> Self {
        self.fp_rebuild = rebuild;
        self.fp_secret = secret;
        self.fp_rv = resource_version;
        self.sample_endpoint_coverage = true;
        self
    }

    /// Attach the owning binary's transition reporter before initialization.
    /// Controller construction deliberately leaves this absent.
    pub fn with_health_transition_reporter(mut self, reporter: HealthTransitionReporter) -> Self {
        self.healthy = Arc::new(PublishedHealth::with_reporter(self.healthy.load(), reporter));
        self
    }

    pub fn apply_fingerprint_parts(&self) -> ([u8; 32], [u8; 32], Option<String>) {
        (self.fp_rebuild, self.fp_secret, self.fp_rv.clone())
    }

    pub fn endpoint_coverage(&self) -> (usize, usize) {
        (
            self.endpoint_live.load(Ordering::Relaxed),
            self.connection.endpoints.len(),
        )
    }

    pub fn mark_retired(&self) {
        let mut retired = self.retired.lock();
        *retired = true;
        self.healthy.deactivate();
        let _ = self.shutdown_tx.send(true);
        if let Some(handle) = self.monitor_handle.lock().take() {
            handle.abort();
        }
    }

    /// Initialize connection and start background health monitor.
    ///
    /// Sets up the etcd connection and spawns a background task that monitors
    /// connection health and reconnects with exponential backoff on failure.
    pub async fn init(&self) -> Result<()> {
        anyhow::ensure!(!*self.retired.lock(), "Etcd client is retired");
        let client = connect_with_options(&self.connection.endpoints, &self.connection.options.ordinary)
            .await
            .map_err(|e| anyhow::anyhow!("Etcd [{}] connect failed: {:?}", self.name, e))?;

        {
            let mut guard = self.client.write().await;
            let retired = self.retired.lock();
            anyhow::ensure!(!*retired, "Etcd client is retired");
            *guard = Some(client);
            self.healthy.update(true, false);
        }
        if self.sample_endpoint_coverage {
            let live = sample_endpoint_live(
                &self.connection.endpoints,
                &self.connection.options.ordinary,
                self.connection.probe_timeout,
            )
            .await;
            self.endpoint_live.store(live, Ordering::Relaxed);
        }
        tracing::info!(etcd = %self.name, "Etcd client initialized successfully");

        // Start background health monitor + reconnect task
        self.start_health_monitor();

        Ok(())
    }

    /// Start background task that monitors health and reconnects on failure.
    fn start_health_monitor(&self) {
        let client = self.client.clone();
        let connection = Arc::clone(&self.connection);
        let name = self.name.clone();
        let healthy = self.healthy.clone();
        let retired = self.retired.clone();
        let endpoint_live = self.endpoint_live.clone();
        let sample_endpoint_coverage = self.sample_endpoint_coverage;
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        let handle = tokio::spawn(async move {
            let base_interval = Duration::from_secs(10);
            let mut check_interval = base_interval;
            let mut backoff = Duration::from_millis(500);
            let max_backoff = Duration::from_secs(30);
            let probe_timeout = connection.probe_timeout;

            loop {
                if *retired.lock() {
                    return;
                }
                tokio::select! {
                    _ = tokio::time::sleep(check_interval) => {}
                    _ = shutdown_rx.changed() => {
                        tracing::info!(etcd = %name, "Etcd health monitor shutting down");
                        return;
                    }
                }
                if *retired.lock() {
                    return;
                }

                // Try a simple status call as health check. Clone the client
                // INSIDE the read lock, then drop the guard BEFORE awaiting so a
                // stuck etcd never holds the lock across `status()`. The call is
                // bounded by `probe_timeout`; a timeout counts as unhealthy.
                let is_healthy = {
                    let maybe_client = {
                        let guard = client.read().await;
                        guard.as_ref().cloned()
                    };
                    match maybe_client {
                        Some(mut mc) => matches!(tokio::time::timeout(probe_timeout, mc.status()).await, Ok(Ok(_))),
                        None => false,
                    }
                };

                if *retired.lock() {
                    return;
                }

                if sample_endpoint_coverage {
                    let live =
                        sample_endpoint_live(&connection.endpoints, &connection.options.ordinary, probe_timeout).await;
                    endpoint_live.store(live, Ordering::Relaxed);
                }

                if *retired.lock() {
                    return;
                }

                if is_healthy {
                    if healthy.update(true, false) {
                        tracing::info!(etcd = %name, "Etcd connection recovered");
                    }
                    check_interval = base_interval;
                    backoff = Duration::from_millis(500);
                } else {
                    healthy.update(false, false);
                    tracing::warn!(
                        etcd = %name,
                        backoff_ms = backoff.as_millis(),
                        "Etcd unhealthy, attempting reconnect"
                    );

                    // Attempt reconnect
                    match connect_with_options(&connection.endpoints, &connection.options.ordinary).await {
                        Ok(new_client) => {
                            if *retired.lock() {
                                return;
                            }
                            let mut guard = client.write().await;
                            let retired = retired.lock();
                            if *retired {
                                return;
                            }
                            *guard = Some(new_client);
                            healthy.update(true, false);
                            tracing::info!(etcd = %name, "Etcd reconnected successfully");
                            check_interval = base_interval;
                            backoff = Duration::from_millis(500);
                        }
                        Err(e) => {
                            tracing::warn!(
                                etcd = %name,
                                error = %e,
                                "Etcd reconnect failed"
                            );
                            // Apply +/-20% jitter to the wait so that N gateway pods
                            // sharing this etcd cluster do not wake and reconnect in
                            // lockstep on a shared outage (thundering herd). Only the
                            // sleep is jittered; `backoff` keeps doubling deterministically.
                            let jitter = rand::rng().random_range(0.8_f64..=1.2);
                            check_interval = backoff.mul_f64(jitter).min(max_backoff);
                            backoff = (backoff * 2).min(max_backoff);
                        }
                    }
                }
            }
        });

        // Serialize monitor ownership with retirement, including init/shutdown races.
        let retired = self.retired.lock();
        if *retired {
            handle.abort();
        } else {
            *self.monitor_handle.lock() = Some(handle);
        }
    }

    /// Check if the client is connected and healthy.
    #[inline]
    pub fn healthy(&self) -> bool {
        self.healthy.load()
    }

    pub fn published_health(&self) -> &PublishedHealth {
        &self.healthy
    }

    /// Get client name (namespace/name).
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Captured scalar health timeout; reconnect never reparses the config.
    #[inline]
    pub(crate) fn probe_timeout(&self) -> Duration {
        self.connection.probe_timeout
    }

    /// Get namespace prefix (if configured).
    #[inline]
    pub fn namespace(&self) -> Option<&str> {
        self.namespace.as_deref()
    }

    /// Build the full key with namespace prefix.
    pub(crate) fn full_key(&self, key: &str) -> String {
        match &self.namespace {
            Some(ns) => format!("{}{}", ns, key),
            None => key.to_string(),
        }
    }

    /// Get a clone of the inner client for operations.
    /// Returns None if not connected.
    pub(crate) async fn get_client(&self) -> Option<etcd_client::Client> {
        self.client.read().await.clone()
    }

    /// Build an `ElectionSession` on a DEDICATED etcd connection (spec §3.2/§4.2).
    ///
    /// The election must NOT ride the shared conf_center client, because if the
    /// operator configured `EtcdConfig.timeout.request` that client carries a
    /// channel-wide request timeout, and v3election `campaign()` blocks
    /// server-side until won — a request timeout would abort the blocking
    /// campaign every cycle and break active-passive HA. The saved election options
    /// omit the request timeout while keeping h2 keepalive on (so a blackholed
    /// campaign connection still dies) and the same endpoints/auth/TLS. Using a
    /// separate connection also sidesteps the shared client's health-monitor
    /// swap (§4.6) for the campaign/resign path.
    pub async fn election_session(&self) -> Result<crate::link_sys_core::providers::etcd::election::ElectionSession> {
        let client = etcd_client::Client::connect(
            &self.connection.endpoints,
            Some(self.connection.options.election.clone()),
        )
        .await
        .map_err(|e| anyhow::anyhow!("Etcd [{}] election connect: {:?}", self.name, e))?;
        Ok(crate::link_sys_core::providers::etcd::election::ElectionSession::new(
            client,
        ))
    }

    /// Graceful shutdown — signal health monitor to stop and clear the client.
    pub async fn shutdown(&self) -> Result<()> {
        if self.shutdown_started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        tracing::info!(etcd = %self.name, "Etcd client shutting down");
        self.mark_retired();
        // Cooperative exit for the common case: a task parked at the select
        // sleep wakes on the watch change and logs its own shutdown line.
        let _ = self.shutdown_tx.send(true);
        // Request cancellation without joining. An already-running poll may
        // continue until it yields; abort is not a no-write-after-retirement
        // barrier. Network calls hold no client lock, and health reporting was
        // deactivated by mark_retired before cleanup began.
        if let Some(handle) = self.monitor_handle.lock().take() {
            handle.abort();
        }
        let mut guard = self.client.write().await;
        *guard = None;
        self.healthy.update(false, false);
        self.endpoint_live.store(0, Ordering::Relaxed);
        Ok(())
    }
}

/// Initial, recovery and probe connections share the same fail-closed options.
async fn connect_with_options(
    endpoints: &[String],
    options: &etcd_client::ConnectOptions,
) -> Result<etcd_client::Client> {
    Ok(etcd_client::Client::connect(endpoints, Some(options.clone())).await?)
}

async fn sample_endpoint_live(
    endpoints: &[String],
    options: &etcd_client::ConnectOptions,
    probe_timeout: Duration,
) -> usize {
    let mut live = 0;
    for endpoint in endpoints {
        let reachable = match tokio::time::timeout(
            probe_timeout,
            connect_with_options(std::slice::from_ref(endpoint), options),
        )
        .await
        {
            Ok(Ok(mut client)) => matches!(tokio::time::timeout(probe_timeout, client.status()).await, Ok(Ok(_))),
            _ => false,
        };
        if reachable {
            live += 1;
        }
    }
    live
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys_core/providers/etcd/client_test.rs"]
mod tests;
