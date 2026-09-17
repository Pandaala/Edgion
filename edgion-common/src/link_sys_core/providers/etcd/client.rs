//! EtcdLinkClient — runtime Etcd client wrapper.
//!
//! Built from LinkSys CRD config. Gateway LinkSys owns construct / init / retire
//! through the persistent runner; Controller conf-center still calls
//! `from_config` + `init` directly.
//! Wraps `etcd_client::Client` with:
//! - Auto-reconnect via background health monitor (etcd-client has no built-in reconnect)
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

use crate::link_sys_core::credential::CredentialState;
use edgion_resources::credentials::materials::BasicAuthMaterial;
use edgion_resources::resources::link_sys::etcd::EtcdClientConfig;

use super::config_mapping::{build_connect_options, default_connect_options, health_probe_timeout};

/// Runtime Etcd client wrapper.
///
/// One published generation for Gateway LinkSys, or the Controller conf-center
/// client when constructed without apply metadata.
pub struct EtcdLinkClient {
    /// etcd client (wrapped in RwLock for reconnect swaps)
    client: Arc<RwLock<Option<etcd_client::Client>>>,
    /// Original config (kept for reconnection)
    config: EtcdClientConfig,
    /// Basic auth from LinkSys runtime (reconnect)
    auth_basic: Option<BasicAuthMaterial>,
    /// Parsed endpoints (kept for reconnection)
    endpoints: Vec<String>,
    /// Human-readable name ("namespace/name")
    name: String,
    /// Atomic health flag
    healthy: Arc<AtomicBool>,
    /// Namespace prefix (if configured)
    namespace: Option<String>,
    /// Shutdown signal sender
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// Handle to the spawned health-monitor task (populated by `init()`),
    /// aborted in `shutdown()` so the task is deterministically terminated.
    monitor_handle: parking_lot::Mutex<Option<JoinHandle<()>>>,
    /// Apply fingerprint hashes captured by the Gateway runner (D19).
    fp_rebuild: [u8; 32],
    fp_secret: [u8; 32],
    fp_rv: Option<String>,
    /// Set before handover retire so the monitor stops mutating shared health.
    retired: Arc<AtomicBool>,
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
            .field("endpoints", &self.endpoints)
            .field("healthy", &self.healthy.load(Ordering::Relaxed))
            .field("namespace", &self.namespace)
            .finish()
    }
}

impl EtcdLinkClient {
    /// Create from CRD config. Does NOT connect — call `init()` next.
    ///
    /// Dispatch is expected to filter out `Pending` / `Invalid` states by
    /// checking `auth.is_buildable() && credential::tls_ready(ls)` before
    /// calling; reaching this function with a non-buildable state is treated
    /// as an internal bug and fails fast.
    pub fn from_config(
        name: &str,
        config: &EtcdClientConfig,
        auth: &CredentialState<BasicAuthMaterial>,
    ) -> Result<Self> {
        if config.endpoints.is_empty() {
            anyhow::bail!("Etcd endpoints list is empty for '{}'", name);
        }

        // Extract the owned material for the reconnect path. `NotRequired` /
        // `Resolved` are the only states we expect here; anything else would
        // have been filtered out by the dispatcher.
        let auth_basic = match auth {
            CredentialState::NotRequired => None,
            CredentialState::Resolved(m) => Some(m.clone()),
            CredentialState::Pending | CredentialState::Invalid(_) => {
                anyhow::bail!(
                    "Etcd [{}] from_config called with non-buildable credential state: {:?}",
                    name,
                    auth
                );
            }
        };

        let (shutdown_tx, _) = tokio::sync::watch::channel(false);

        Ok(Self {
            client: Arc::new(RwLock::new(None)),
            config: config.clone(),
            auth_basic,
            endpoints: config.endpoints.clone(),
            name: name.to_string(),
            healthy: Arc::new(AtomicBool::new(false)),
            namespace: config.namespace.clone(),
            shutdown_tx,
            monitor_handle: parking_lot::Mutex::new(None),
            fp_rebuild: [0; 32],
            fp_secret: [0; 32],
            fp_rv: None,
            retired: Arc::new(AtomicBool::new(false)),
            shutdown_started: AtomicBool::new(false),
            endpoint_live: Arc::new(AtomicUsize::new(0)),
            sample_endpoint_coverage: false,
        })
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

    pub fn apply_fingerprint_parts(&self) -> ([u8; 32], [u8; 32], Option<String>) {
        (self.fp_rebuild, self.fp_secret, self.fp_rv.clone())
    }

    pub fn endpoint_coverage(&self) -> (usize, usize) {
        (self.endpoint_live.load(Ordering::Relaxed), self.endpoints.len())
    }

    pub fn mark_retired(&self) {
        self.retired.store(true, Ordering::Relaxed);
        let _ = self.shutdown_tx.send(true);
        if let Some(handle) = self.monitor_handle.lock().take() {
            handle.abort();
        }
    }

    #[cfg(test)]
    pub(crate) fn samples_endpoint_coverage(&self) -> bool {
        self.sample_endpoint_coverage
    }

    /// Rebuild a `CredentialState` from the stored material.
    ///
    /// Used by `init()` / reconnect to feed `build_connect_options` which now
    /// takes `&CredentialState<_>`. Since we only ever store the result of a
    /// `NotRequired` or `Resolved` state, the reconstruction round-trips.
    fn auth_state(&self) -> CredentialState<BasicAuthMaterial> {
        match &self.auth_basic {
            Some(m) => CredentialState::Resolved(m.clone()),
            None => CredentialState::NotRequired,
        }
    }

    /// Initialize connection and start background health monitor.
    ///
    /// Sets up the etcd connection and spawns a background task that monitors
    /// connection health and reconnects with exponential backoff on failure.
    pub async fn init(&self) -> Result<()> {
        let options = build_connect_options(&self.config, &self.auth_state())?;

        // If no options were configured, use defaults (with dial timeout)
        let connect_options = options.unwrap_or_else(default_connect_options);

        // Connect to etcd
        let client = etcd_client::Client::connect(&self.endpoints, Some(connect_options))
            .await
            .map_err(|e| anyhow::anyhow!("Etcd [{}] connect failed: {:?}", self.name, e))?;

        {
            let mut guard = self.client.write().await;
            *guard = Some(client);
        }
        self.healthy.store(true, Ordering::Relaxed);
        if self.sample_endpoint_coverage {
            let live = sample_endpoint_live(
                &self.endpoints,
                &self.config,
                &self.auth_state(),
                health_probe_timeout(&self.config),
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
        let config = self.config.clone();
        let auth_state = self.auth_state();
        let endpoints = self.endpoints.clone();
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
            // Resolved once: `config` is immutable for the life of the task.
            let probe_timeout = health_probe_timeout(&config);

            loop {
                if retired.load(Ordering::Relaxed) {
                    return;
                }
                tokio::select! {
                    _ = tokio::time::sleep(check_interval) => {}
                    _ = shutdown_rx.changed() => {
                        tracing::info!(etcd = %name, "Etcd health monitor shutting down");
                        return;
                    }
                }
                if retired.load(Ordering::Relaxed) {
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

                if retired.load(Ordering::Relaxed) {
                    return;
                }

                if sample_endpoint_coverage {
                    let live = sample_endpoint_live(&endpoints, &config, &auth_state, probe_timeout).await;
                    endpoint_live.store(live, Ordering::Relaxed);
                }

                if retired.load(Ordering::Relaxed) {
                    return;
                }

                if is_healthy {
                    if !healthy.load(Ordering::Relaxed) {
                        tracing::info!(etcd = %name, "Etcd connection recovered");
                    }
                    healthy.store(true, Ordering::Relaxed);
                    check_interval = base_interval;
                    backoff = Duration::from_millis(500);
                } else {
                    healthy.store(false, Ordering::Relaxed);
                    tracing::warn!(
                        etcd = %name,
                        backoff_ms = backoff.as_millis(),
                        "Etcd unhealthy, attempting reconnect"
                    );

                    // Attempt reconnect
                    let options = build_connect_options(&config, &auth_state).ok().flatten();
                    let connect_options = options.unwrap_or_else(default_connect_options);
                    match etcd_client::Client::connect(&endpoints, Some(connect_options)).await {
                        Ok(new_client) => {
                            if retired.load(Ordering::Relaxed) {
                                return;
                            }
                            let mut guard = client.write().await;
                            *guard = Some(new_client);
                            healthy.store(true, Ordering::Relaxed);
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

        *self.monitor_handle.lock() = Some(handle);
    }

    /// Check if the client is connected and healthy.
    #[inline]
    pub fn healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    /// Get client name (namespace/name).
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Borrow the original CRD config (used by the ops module for
    /// health-probe timeout resolution; the field itself is module-private).
    #[inline]
    pub(crate) fn config(&self) -> &EtcdClientConfig {
        &self.config
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

    /// Build an [`ElectionSession`] on a Client configured for a BLOCKING
    /// Build an `ElectionSession` on a DEDICATED etcd connection (spec §3.2/§4.2).
    ///
    /// The election must NOT ride the shared conf_center client, because if the
    /// operator configured `EtcdConfig.timeout.request` that client carries a
    /// channel-wide request timeout, and v3election `campaign()` blocks
    /// server-side until won — a request timeout would abort the blocking
    /// campaign every cycle and break active-passive HA. `election_connect_options`
    /// strips the request timeout while keeping h2 keepalive on (so a blackholed
    /// campaign connection still dies) and the same endpoints/auth/TLS. Using a
    /// separate connection also sidesteps the shared client's health-monitor
    /// swap (§4.6) for the campaign/resign path.
    pub async fn election_session(&self) -> Result<crate::link_sys_core::providers::etcd::election::ElectionSession> {
        let opts = super::config_mapping::election_connect_options(&self.config, &self.auth_state())?;
        let client = etcd_client::Client::connect(&self.endpoints, Some(opts))
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
        // Hard backstop: abort the monitor so a task mid `status()` / `connect()`
        // (holding the `client` read lock) is cancelled at its next await point,
        // releasing the read lock so the `client.write()` below cannot block for
        // the full etcd call timeout. The task holds no unwind-critical state, so
        // abort (no join) is sufficient — same as the Webhook health_task path.
        if let Some(handle) = self.monitor_handle.lock().take() {
            handle.abort();
        }
        let mut guard = self.client.write().await;
        *guard = None;
        self.healthy.store(false, Ordering::Relaxed);
        self.endpoint_live.store(0, Ordering::Relaxed);
        Ok(())
    }
}

async fn sample_endpoint_live(
    endpoints: &[String],
    config: &EtcdClientConfig,
    auth_state: &CredentialState<BasicAuthMaterial>,
    probe_timeout: Duration,
) -> usize {
    let mut live = 0;
    for endpoint in endpoints {
        let options = build_connect_options(config, auth_state)
            .ok()
            .flatten()
            .unwrap_or_else(default_connect_options);
        let reachable = match tokio::time::timeout(
            probe_timeout,
            etcd_client::Client::connect(std::slice::from_ref(endpoint), Some(options)),
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
mod tests {
    use super::*;

    fn minimal_config() -> EtcdClientConfig {
        EtcdClientConfig {
            endpoints: vec!["http://localhost:2379".to_string()],
            auth: None,
            tls: None,
            timeout: None,
            keep_alive: None,
            namespace: None,
            auto_sync_interval: None,
            max_call_send_size: None,
            max_call_recv_size: None,
            user_agent: None,
            reject_old_cluster: None,
            observability: None,
        }
    }

    fn no_auth() -> CredentialState<BasicAuthMaterial> {
        CredentialState::NotRequired
    }

    #[test]
    fn test_from_config_minimal() {
        let config = minimal_config();
        let client = EtcdLinkClient::from_config("default/etcd-test", &config, &no_auth());
        assert!(client.is_ok());
        let client = client.unwrap();
        assert_eq!(client.name(), "default/etcd-test");
        assert!(!client.healthy());
        assert!(client.namespace().is_none());
    }

    #[test]
    fn test_from_config_with_namespace() {
        let config = EtcdClientConfig {
            namespace: Some("/app/".to_string()),
            ..minimal_config()
        };
        let client = EtcdLinkClient::from_config("default/etcd-ns", &config, &no_auth()).unwrap();
        assert_eq!(client.namespace(), Some("/app/"));
        assert_eq!(client.full_key("mykey"), "/app/mykey");
    }

    #[test]
    fn test_from_config_without_namespace() {
        let config = minimal_config();
        let client = EtcdLinkClient::from_config("default/etcd-test", &config, &no_auth()).unwrap();
        assert_eq!(client.full_key("mykey"), "mykey");
    }

    #[test]
    fn test_empty_endpoints_returns_error() {
        let config = EtcdClientConfig {
            endpoints: vec![],
            ..minimal_config()
        };
        let result = EtcdLinkClient::from_config("test", &config, &no_auth());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    /// Defence-in-depth: `from_config` must refuse `Pending` / `Invalid` even
    /// though dispatch normally filters these out up-front.
    #[test]
    fn test_from_config_pending_state_fails_fast() {
        let config = minimal_config();
        let result = EtcdLinkClient::from_config("default/etcd-test", &config, &CredentialState::Pending);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("non-buildable"));
    }

    #[test]
    fn controller_construction_does_not_enable_endpoint_coverage_sampling() {
        let client = EtcdLinkClient::from_config("default/etcd-test", &minimal_config(), &no_auth()).unwrap();
        assert!(!client.samples_endpoint_coverage());
        assert_eq!(client.endpoint_coverage(), (0, 1));
        let (rebuild, secret, rv) = client.apply_fingerprint_parts();
        assert_eq!(rebuild, [0; 32]);
        assert_eq!(secret, [0; 32]);
        assert!(rv.is_none());
    }

    #[test]
    fn gateway_apply_meta_attaches_fingerprint_and_enables_sampling() {
        let rebuild = [7u8; 32];
        let secret = [9u8; 32];
        let client = EtcdLinkClient::from_config("default/etcd-gw", &minimal_config(), &no_auth())
            .unwrap()
            .with_gateway_apply_meta(rebuild, secret, Some("11".to_string()));
        assert!(client.samples_endpoint_coverage());
        assert_eq!(
            client.apply_fingerprint_parts(),
            (rebuild, secret, Some("11".to_string()))
        );
        assert_eq!(client.endpoint_coverage(), (0, 1));
    }
}
