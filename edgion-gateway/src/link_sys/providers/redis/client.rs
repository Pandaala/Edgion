//! Published Redis generation. The persistent runner owns initial retry and handover.
use super::{
    backend::RedisBackend,
    resolver::{redis_rs_resolver, RedisEgressResolver},
    transport::Transport,
};
use crate::link_sys::runtime::apply::{ConnectionCoverage, RuntimeFingerprint};
use crate::link_sys::runtime::health_probe::{new_published_health, spawn_health_probe, ProbeConfig};
use crate::observe::metrics::registry::{record_link_sys_redis_topology_event, RedisTopologyEvent};
use anyhow::Result;
use edgion_common::link_sys_core::PublishedHealth;
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};
use rand::Rng;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Partial or failed recovery must keep backing off. Only a complete recovery
/// returns to the 100 ms base; the caller applies jitter and the 30 s cap.
pub(super) fn next_recovery_delay(current: Duration, fully_recovered: bool) -> Duration {
    if fully_recovered {
        Duration::from_millis(100)
    } else {
        (current * 2).min(Duration::from_secs(30))
    }
}

pub struct RedisLinkClient {
    pub(super) backend: Arc<RedisBackend>,
    name: String,
    fingerprint: RuntimeFingerprint,
    shutdown_started: AtomicBool,
    healthy: Arc<PublishedHealth>,
    shutdown_tx: parking_lot::Mutex<Option<watch::Sender<bool>>>,
    tasks: parking_lot::Mutex<Vec<JoinHandle<()>>>,
    pub(super) resolver: std::sync::OnceLock<RedisEgressResolver>,
}
impl RedisLinkClient {
    /// Create from the actual resolved resource. Does not connect; call init next.
    pub fn from_link_sys(owner: Arc<LinkSys>) -> Result<Self> {
        let candidate = super::apply::RedisCandidate::new(owner)?;
        let owner = candidate.tls.owner();
        let name = format!(
            "{}/{}",
            owner.metadata.namespace.as_deref().unwrap_or("default"),
            owner.metadata.name.as_deref().unwrap_or_default(),
        );
        Self::from_candidate(&name, &candidate)
    }

    pub(super) fn from_candidate(name: &str, candidate: &super::apply::RedisCandidate) -> Result<Self> {
        let SystemConfig::Redis(config) = &candidate.tls.owner().spec.config else {
            anyhow::bail!("Redis construction requires an actual Redis owner");
        };
        let auth = crate::link_sys::credential::basic_state(candidate.tls.owner());
        let transport = Transport::new(config, &auth, candidate.tls.redis_tls_material()?)?;
        Ok(Self {
            backend: Arc::new(RedisBackend::new(transport)),
            name: name.to_owned(),
            fingerprint: candidate.fingerprint().clone(),
            shutdown_started: AtomicBool::new(false),
            healthy: new_published_health(name, false),
            shutdown_tx: parking_lot::Mutex::new(None),
            tasks: parking_lot::Mutex::new(Vec::new()),
            resolver: std::sync::OnceLock::new(),
        })
    }
    pub(crate) fn runtime_fingerprint(&self) -> &RuntimeFingerprint {
        &self.fingerprint
    }
    pub(crate) fn connection_coverage(&self) -> ConnectionCoverage {
        let (live, configured) = *self.backend.coverage.lock();
        ConnectionCoverage { live, configured }
    }
    pub(crate) fn published_health(&self) -> &PublishedHealth {
        &self.healthy
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn healthy(&self) -> bool {
        self.healthy.load()
    }
    pub(crate) fn mark_retired(&self) {
        self.healthy.deactivate();
        self.backend.retire();
        if let Some(tx) = self.shutdown_tx.lock().as_ref() {
            let _ = tx.send(true);
        }
    }
    pub async fn init(&self) -> Result<()> {
        self.initialize(redis_rs_resolver()).await
    }
    #[cfg(feature = "test-util")]
    pub async fn init_for_test(&self) -> Result<()> {
        self.initialize(super::resolver::redis_rs_resolver_for_test()).await
    }
    async fn initialize(&self, resolver: RedisEgressResolver) -> Result<()> {
        self.resolver
            .set(resolver.clone())
            .map_err(|_| anyhow::anyhow!("Redis already initialized"))?;
        self.backend
            .establish(resolver.clone())
            .await
            .map_err(|error| anyhow::anyhow!(initialization_failure_message(&error)))?;
        anyhow::ensure!(
            self.backend.probe(resolver.clone()).await,
            "Redis initialization health check failed"
        );
        self.healthy.update(true, false);
        let (tx, rx) = watch::channel(false);
        *self.shutdown_tx.lock() = Some(tx);
        let backend = self.backend.clone();
        let probe_resolver = resolver.clone();
        let probe = spawn_health_probe(
            ProbeConfig {
                name: self.name.clone(),
                system_type: "redis",
                base_interval: Duration::from_secs(5),
                max_backoff: Duration::from_secs(5),
                probe_timeout: Duration::from_secs(3),
                upgrade_only: false,
            },
            self.healthy.clone(),
            rx.clone(),
            move || {
                let backend = backend.clone();
                let resolver = probe_resolver.clone();
                async move { backend.probe(resolver).await }
            },
        );
        let backend = self.backend.clone();
        let mut shutdown = rx;
        let name = self.name.clone();
        let health = self.healthy.clone();
        let recovery = tokio::spawn(async move {
            let mut delay = Duration::from_millis(100);
            // Log transitions only, with metadata (link name), never endpoints
            // or dependency error text; repeated failures stay silent.
            let mut failing = false;
            loop {
                tokio::select! { biased;
                    _ = shutdown.changed() => break,
                    _ = backend.wake.notified() => {},
                }
                while backend.needs_recovery.load(Ordering::Acquire) {
                    let wait = delay
                        .mul_f64(rand::rng().random_range(0.8..=1.2))
                        .min(Duration::from_secs(30));
                    tokio::select! { biased;
                        _ = shutdown.changed() => return,
                        _ = tokio::time::sleep(wait) => {},
                    }
                    let primary_before = backend.published_primary();
                    let result = tokio::select! { biased;
                        _ = shutdown.changed() => return,
                        result = backend.establish(resolver.clone()) => result,
                    };
                    match &result {
                        Ok(()) => {
                            if failing {
                                failing = false;
                                tracing::info!(redis = %name, "Redis topology recovery succeeded");
                                report_topology_event(&health, &name, RedisTopologyEvent::Recovered);
                            }
                            if primary_changed(primary_before.as_ref(), backend.published_primary().as_ref()) {
                                tracing::info!(redis = %name, "Redis Sentinel primary changed");
                                report_topology_event(&health, &name, RedisTopologyEvent::PrimaryChanged);
                            }
                        }
                        Err(_) if !failing => {
                            failing = true;
                            tracing::warn!(redis = %name, "Redis topology recovery failed; retrying");
                            report_topology_event(&health, &name, RedisTopologyEvent::RecoveryFailed);
                        }
                        Err(_) => {}
                    }
                    let fully_recovered = result.is_ok() && !backend.needs_recovery.load(Ordering::Acquire);
                    delay = next_recovery_delay(delay, fully_recovered);
                }
            }
        });
        self.tasks.lock().extend([probe, recovery]);
        Ok(())
    }
    /// Rebuild the published connection from the current known topology.
    /// Test-only: production recovery owns this path.
    #[cfg(any(test, feature = "test-util"))]
    pub async fn reestablish_for_test(&self) -> Result<()> {
        let resolver = self
            .resolver
            .get()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Redis is not initialized"))?;
        self.backend
            .establish(resolver)
            .await
            .map_err(|_| anyhow::anyhow!("Redis reestablish failed"))
    }
    pub(crate) fn operation_deadline(&self) -> tokio::time::Instant {
        tokio::time::Instant::now() + self.backend.transport.command_timeout
    }
    pub(crate) async fn exec_until(
        &self,
        cmd: &redis::Cmd,
        deadline: tokio::time::Instant,
    ) -> redis::RedisResult<redis::Value> {
        let resolver = self.resolver.get().ok_or_else(super::backend::unavailable)?.clone();
        self.backend.exec(cmd, deadline, resolver).await
    }
    pub(crate) async fn exec(&self, cmd: &redis::Cmd) -> redis::RedisResult<redis::Value> {
        self.exec_until(cmd, self.operation_deadline()).await
    }
    pub async fn shutdown(&self) -> Result<()> {
        if self.shutdown_started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.mark_retired();
        let tasks = std::mem::take(&mut *self.tasks.lock());
        for task in tasks {
            let _ = task.await;
        }
        self.backend.snapshot.store(None);
        self.healthy.update(false, false);
        Ok(())
    }
}

/// Count a topology event only for the published generation, matching the
/// health flip counter: candidates and retired generations never report.
fn report_topology_event(health: &PublishedHealth, name: &str, event: RedisTopologyEvent) {
    health.report_if_published(|| record_link_sys_redis_topology_event(name, event));
}

/// A failover is a published Sentinel primary replaced by a different one.
/// A first publication or a removed snapshot is not a primary change.
pub(super) fn primary_changed(before: Option<&(String, u16)>, after: Option<&(String, u16)>) -> bool {
    match (before, after) {
        (Some(before), Some(after)) => {
            super::backend::endpoint_identity(before) != super::backend::endpoint_identity(after)
        }
        _ => false,
    }
}

/// Keep dial failures fixed. A certificate rejection is distinguishable from a
/// plaintext or refused connection without copying the peer address into the log.
pub(super) fn initialization_failure_message(error: &redis::RedisError) -> &'static str {
    if super::backend::is_tls_verification_error(error) {
        "Redis TLS verification failed"
    } else {
        "Redis initialization failed"
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/redis/client_test/test_util.rs"]
mod test_util;
#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/redis/client_test.rs"]
mod tests;

#[cfg(all(not(test), feature = "test-util"))]
#[path = "../../../../tests/unit/link_sys/providers/redis/client_test/test_util.rs"]
mod test_util;
