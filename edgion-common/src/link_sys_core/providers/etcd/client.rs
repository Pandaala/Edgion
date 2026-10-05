//! EtcdLinkClient — runtime Etcd client wrapper.
//!
//! Built from LinkSys CRD config. Gateway LinkSys owns construct / init / retire
//! through the persistent runner; Controller conf-center still calls
//! `from_config` + `init` directly.
//! Wraps `etcd_client::Client` with:
//! - Connection-bound admission and recovery through the patched SDK endpoint set
//! - Health tracking via AtomicBool
//! - Namespace prefix support for key isolation
//!
//! Unlike fred (Redis), etcd-client uses gRPC multiplexing — a single connection
//! handles multiple concurrent streams, so no connection pool is needed.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use rand::Rng;
use tokio::sync::RwLock;
use tokio::task::AbortHandle;

use crate::link_sys_core::credential::CredentialState;
use crate::link_sys_core::{HealthTransitionReporter, PublishedHealth};
use edgion_resources::credentials::materials::BasicAuthMaterial;
use edgion_resources::resources::link_sys::etcd::EtcdClientConfig;

use super::observation::{
    CompletionBridge, EtcdObservation, MaintenanceEvent, MaintenanceKind, MaintenanceOutcome, Operation,
};
use super::{dialer::EtcdDialer, policy::EtcdRuntimePolicy};
use crate::egress::EgressPolicy;
use etcd_client::transport::{AdmissionControl, AdmittedEndpointSet};

/// Runtime Etcd client wrapper.
///
/// One published generation for Gateway LinkSys, or the Controller conf-center
/// client when constructed without apply metadata.
pub struct EtcdLinkClient {
    /// Published facade, cleared on shutdown; endpoint recovery stays inside the SDK set.
    client: Arc<RwLock<Option<etcd_client::Client>>>,
    /// Immutable connection policy for this generation.
    policy: Arc<EtcdRuntimePolicy>,
    generation: AdmissionControl,
    observation: Option<Arc<dyn EtcdObservation>>,
    observe_completions: bool,
    endpoint_set: parking_lot::Mutex<Option<AdmittedEndpointSet>>,
    egress_policy: EgressPolicy,
    dns_cache_ttl: Duration,
    /// Bootstrap endpoint labels retained for debug inspection.
    endpoints: Vec<String>,
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
    monitor_handle: parking_lot::Mutex<Option<AbortHandle>>,
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
            .field("healthy", &self.healthy.load())
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
        let validation_errors = config.validation_errors();
        if !validation_errors.is_empty() {
            anyhow::bail!("Invalid Etcd configuration: {}", validation_errors.join("; "));
        }

        // Validate the credential state before resolving owned policy. `NotRequired` /
        // `Resolved` are the only states we expect here; anything else would
        // have been filtered out by the dispatcher.
        match auth {
            CredentialState::NotRequired | CredentialState::Resolved(_) => {}
            _ => anyhow::bail!("Etcd from_config called with non-buildable credential state"),
        }
        let policy = Arc::new(EtcdRuntimePolicy::resolve(config, auth)?);

        let (shutdown_tx, _) = tokio::sync::watch::channel(false);

        Ok(Self {
            client: Arc::new(RwLock::new(None)),
            policy,
            generation: AdmissionControl::new(None),
            observation: None,
            observe_completions: false,
            endpoint_set: parking_lot::Mutex::new(None),
            egress_policy: EgressPolicy::default(),
            dns_cache_ttl: Duration::from_secs(30),
            endpoints: config.endpoints.clone(),
            name: name.to_string(),
            healthy: Arc::new(PublishedHealth::new(false)),
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
        let total = self
            .endpoint_set
            .lock()
            .as_ref()
            .map_or(self.policy.endpoints.len(), |set| set.coverage().1);
        (self.endpoint_live.load(Ordering::Relaxed), total)
    }

    pub fn mark_retired(&self) {
        if let Some(observer) = &self.observation {
            observer.deactivate();
        }
        self.healthy.deactivate();
        self.generation.cancel();
        self.retired.store(true, Ordering::Relaxed);
        let _ = self.shutdown_tx.send(true);
        if let Some(handle) = self.monitor_handle.lock().take() {
            handle.abort();
        }
    }

    /// Inject the owning binary's canonical egress policy before initialization.
    pub fn with_egress_policy(mut self, policy: EgressPolicy, dns_cache_ttl: Duration) -> Self {
        self.egress_policy = policy;
        self.dns_cache_ttl = dns_cache_ttl;
        self
    }

    /// Install binary-owned maintenance and optional RPC completion observations.
    pub fn with_observation(mut self, observer: Arc<dyn EtcdObservation>, completions: bool) -> Self {
        self.observation = Some(observer);
        self.observe_completions = completions;
        self
    }

    fn new_endpoint_set(&self, election: bool) -> Result<AdmittedEndpointSet> {
        let dialer = Arc::new(EtcdDialer::new(&self.policy, self.egress_policy, self.dns_cache_ttl));
        let mut options = if election {
            self.policy.options.clone().without_timeout()
        } else {
            self.policy.options.clone()
        };
        if self.observe_completions {
            if let Some(observer) = &self.observation {
                options = options.with_observer(Arc::new(CompletionBridge(observer.clone())));
            }
        }
        Ok(AdmittedEndpointSet::new(
            options,
            self.policy.admission.clone(),
            dialer,
            self.generation.clone(),
        )?)
    }

    /// Establish admitted connections, then publish only while this generation lives.
    pub async fn init(&self) -> Result<()> {
        let started = Instant::now();
        let result = self.init_inner().await;
        self.observe_maintenance(MaintenanceKind::Init, started, result.is_ok(), None);
        result
    }

    fn observe_maintenance(&self, kind: MaintenanceKind, started: Instant, ok: bool, health_changed: Option<bool>) {
        report_maintenance(
            self.observation.as_ref(),
            &self.generation,
            kind,
            started,
            ok,
            health_changed,
        );
    }

    async fn init_inner(&self) -> Result<()> {
        let set = self.new_endpoint_set(false)?;
        set.replace_endpoints(self.policy.endpoints.clone()).await?;
        if self.policy.sync_interval.is_some() {
            let started = Instant::now();
            let result = super::discovery::refresh(&set, &self.policy).await;
            self.observe_maintenance(MaintenanceKind::Discovery, started, result.is_ok(), None);
        }
        let client = set.client()?;
        let live = if self.sample_endpoint_coverage {
            set.probe().await
        } else {
            set.coverage().0
        };
        let mut slot = self.client.write().await;
        anyhow::ensure!(
            self.generation
                .while_active(|| {
                    *slot = Some(client);
                    *self.endpoint_set.lock() = Some(set.clone());
                    self.endpoint_live.store(live, Ordering::Relaxed);
                    self.healthy.update(live > 0, false);
                })
                .is_some(),
            "Etcd provider retired during initialization"
        );
        drop(slot);
        self.start_health_monitor(set);
        Ok(())
    }

    /// One provider-owned loop schedules both health and discovery by their
    /// nearest deadline. Discovery never overlaps, and one-second intervals are
    /// not rounded up to the ten-second health cadence.
    fn start_health_monitor(&self, set: AdmittedEndpointSet) {
        let policy = self.policy.clone();
        let observation = self.observation.clone();
        let healthy = self.healthy.clone();
        let generation = self.generation.clone();
        let endpoint_live = self.endpoint_live.clone();
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let handle = tokio::spawn(async move {
            let mut health_due = tokio::time::Instant::now() + Duration::from_secs(10);
            let mut discovery_due = policy
                .sync_interval
                .map(|interval| tokio::time::Instant::now() + interval);
            let mut backoff = Duration::from_millis(500);
            loop {
                if generation.is_cancelled() {
                    return;
                }
                let next = discovery_due.map_or(health_due, |due| due.min(health_due));
                tokio::select! { _ = tokio::time::sleep_until(next) => {}, _ = shutdown_rx.changed() => return }
                if generation.is_cancelled() {
                    return;
                }
                if discovery_due.is_some_and(|due| due <= tokio::time::Instant::now()) {
                    let started = Instant::now();
                    let result = super::discovery::refresh(&set, &policy).await;
                    report_maintenance(
                        observation.as_ref(),
                        &generation,
                        MaintenanceKind::Discovery,
                        started,
                        result.is_ok(),
                        None,
                    );
                    discovery_due = policy.sync_interval.map(|interval| {
                        let jitter = rand::rng().random_range(0.9_f64..=1.1);
                        tokio::time::Instant::now() + interval.mul_f64(jitter)
                    });
                }
                if health_due <= tokio::time::Instant::now() {
                    let started = Instant::now();
                    let live = set.probe().await;
                    let mut changed = false;
                    if generation
                        .while_active(|| {
                            endpoint_live.store(live, Ordering::Relaxed);
                            changed = healthy.update(live > 0, false);
                        })
                        .is_none()
                    {
                        return;
                    }
                    report_maintenance(
                        observation.as_ref(),
                        &generation,
                        MaintenanceKind::Probe,
                        started,
                        live > 0,
                        changed.then_some(live > 0),
                    );
                    let interval = if live > 0 {
                        backoff = Duration::from_millis(500);
                        Duration::from_secs(10)
                    } else {
                        let started = Instant::now();
                        let result = set.reconnect().await;
                        report_maintenance(
                            observation.as_ref(),
                            &generation,
                            MaintenanceKind::Reconnect,
                            started,
                            result.is_ok(),
                            None,
                        );
                        let jitter = rand::rng().random_range(0.8_f64..=1.2);
                        let interval = backoff.mul_f64(jitter).min(Duration::from_secs(30));
                        backoff = (backoff * 2).min(Duration::from_secs(30));
                        interval
                    };
                    health_due = tokio::time::Instant::now() + interval;
                }
            }
        });
        // The handle installation shares the retirement publication barrier.
        if self
            .generation
            .while_active(|| {
                if let Some(previous) = self.monitor_handle.lock().replace(handle.abort_handle()) {
                    previous.abort();
                }
            })
            .is_none()
        {
            handle.abort();
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

    /// Resolved health deadline; no request-path configuration interpretation.
    pub(crate) fn probe_timeout(&self) -> Duration {
        self.policy.probe_timeout
    }

    /// Get namespace prefix (if configured).
    #[inline]
    pub fn namespace(&self) -> Option<&str> {
        self.namespace.as_deref()
    }

    /// Check namespace and borrowed fields before allocating a prefixed key.
    pub(crate) fn checked_full_key(
        &self,
        client: &etcd_client::Client,
        operation: Operation,
        key: &str,
        value_bytes: usize,
        key_copies: usize,
    ) -> Result<String> {
        let key_bytes = self.namespace.as_ref().map_or(0, String::len).saturating_add(key.len());
        client.check_send_size_lower_bound_for(
            operation,
            key_bytes.saturating_mul(key_copies).saturating_add(value_bytes),
        )?;
        Ok(match &self.namespace {
            Some(namespace) => format!("{namespace}{key}"),
            None => key.to_owned(),
        })
    }

    /// Get a clone of the inner client for operations.
    /// Returns None if not connected.
    pub(crate) async fn get_client(&self) -> Option<etcd_client::Client> {
        self.client.read().await.clone()
    }

    /// Build an election session on dedicated, admitted connections.
    ///
    /// Election RPCs omit the unary request timeout because campaign waits until
    /// leadership is won. Keepalive, authentication, TLS, message limits and
    /// generation cancellation remain shared policy. New calls reconcile the
    /// latest discovered topology; an issued campaign is never replayed.
    pub async fn election_session(&self) -> Result<crate::link_sys_core::providers::etcd::election::ElectionSession> {
        let main = self
            .endpoint_set
            .lock()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Etcd client is not initialized"))?;
        let set = main.fork_without_timeout().await?;
        let client = set.client()?;
        Ok(crate::link_sys_core::providers::etcd::election::ElectionSession::new(
            client,
        ))
    }

    /// Graceful shutdown — signal health monitor to stop and clear the client.
    pub async fn shutdown(&self) -> Result<()> {
        if self.shutdown_started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let started = Instant::now();
        self.mark_retired();
        // Cooperative exit for the common case: a task parked at the select
        // sleep wakes on the watch change.
        let _ = self.shutdown_tx.send(true);
        // Request cancellation without joining. An already-running poll may
        // continue until it yields; abort is not a no-write-after-retirement
        // barrier by itself. The generation gate fences publication; network
        // calls hold no client lock, and health reporting was
        // deactivated by mark_retired before cleanup began.
        if let Some(handle) = self.monitor_handle.lock().take() {
            handle.abort();
        }
        let mut guard = self.client.write().await;
        *guard = None;
        self.healthy.update(false, false);
        self.endpoint_live.store(0, Ordering::Relaxed);
        self.observe_maintenance(MaintenanceKind::Shutdown, started, true, None);
        Ok(())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys_core/providers/etcd/client_test.rs"]
mod tests;

fn report_maintenance(
    observer: Option<&Arc<dyn EtcdObservation>>,
    generation: &AdmissionControl,
    kind: MaintenanceKind,
    started: Instant,
    ok: bool,
    health_changed: Option<bool>,
) {
    if let Some(observer) = observer {
        observer.maintenance(MaintenanceEvent {
            kind,
            outcome: if generation.is_cancelled() {
                MaintenanceOutcome::Canceled
            } else if ok {
                MaintenanceOutcome::Ok
            } else {
                MaintenanceOutcome::Failed
            },
            elapsed: started.elapsed(),
            health_changed,
        });
    }
}
