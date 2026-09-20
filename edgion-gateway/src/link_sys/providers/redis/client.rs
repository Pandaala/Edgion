//! RedisLinkClient — runtime Redis client wrapper.
//!
//! Built from LinkSys CRD config. The persistent Redis runner owns construct /
//! init / retire; this type is the published small handle.
//! Wraps a `fred::clients::Pool` which internally manages:
//! - Connection pool (round-robin, configurable size)
//! - Automatic reconnection (exponential backoff)
//! - TLS (rustls)
//! - Cluster slot routing / Sentinel failover

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use fred::clients::Pool;
use fred::interfaces::ClientLike;
use fred::types::Resolve;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::link_sys::credential::CredentialState;
use crate::link_sys::runtime::apply::{ConnectionCoverage, RuntimeFingerprint};
use crate::link_sys::runtime::health_probe::{new_published_health, spawn_health_probe, ProbeConfig};
use edgion_common::link_sys_core::PublishedHealth;
use edgion_resources::credentials::materials::BasicAuthMaterial;
use edgion_resources::resources::link_sys::redis::RedisClientConfig;
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};

use super::config_mapping::{build_fred_config, build_fred_pool};
use super::resolver::redis_egress_resolver;
#[cfg(feature = "test-util")]
use super::resolver::redis_egress_resolver_for_test;

/// Runtime Redis client wrapper.
///
/// This struct wraps a `fred::clients::Pool` (not a single `Client`) to support
/// concurrent access with internal round-robin connection pooling.
///
/// One published generation. The runner may hold a second retiring Arc during
/// Handover; callers always go through `get_redis_client`.
pub struct RedisLinkClient {
    /// fred connection pool (round-robin across connections)
    pool: Pool,
    /// Human-readable name ("namespace/name")
    name: String,
    /// Configured Fred pool member count (`pool.size`, default 8).
    pool_size: usize,
    /// Apply fingerprint captured at construction (D19: last-applied after publish).
    fingerprint: RuntimeFingerprint,
    /// Set before handover retire so the probe stops updating this generation.
    retired: Arc<AtomicBool>,
    /// Makes [`Self::shutdown`] idempotent when delete races handover/replace.
    shutdown_started: AtomicBool,
    /// Atomic aggregate health flag, updated by the connection-state probe.
    healthy: Arc<PublishedHealth>,
    /// Shutdown signal for the aggregate connection-state probe.
    probe_shutdown: parking_lot::Mutex<Option<watch::Sender<bool>>>,
    /// Aggregate connection-state probe task.
    probe_handle: parking_lot::Mutex<Option<JoinHandle<()>>>,
}

fn pool_has_connected_member(pool: &Pool) -> bool {
    any_connected(pool.clients().iter().map(ClientLike::is_connected))
}

fn any_connected(states: impl IntoIterator<Item = bool>) -> bool {
    states.into_iter().any(|connected| connected)
}

impl RedisLinkClient {
    /// Create from CRD config. Does NOT connect — call `init()` next.
    pub fn from_config(
        name: &str,
        config: &RedisClientConfig,
        auth: &CredentialState<BasicAuthMaterial>,
    ) -> Result<Self> {
        let fred_config = build_fred_config(config, auth)?;
        let (builder, pool_size) = build_fred_pool(config, fred_config)?;

        let pool = builder
            .build_pool(pool_size)
            .map_err(|e| anyhow::anyhow!("failed to build Redis pool for {}: {:?}", name, e))?;

        Ok(Self {
            pool,
            name: name.to_string(),
            pool_size,
            fingerprint: RuntimeFingerprint::unspecified(),
            retired: Arc::new(AtomicBool::new(false)),
            shutdown_started: AtomicBool::new(false),
            healthy: new_published_health(name, false),
            probe_shutdown: parking_lot::Mutex::new(None),
            probe_handle: parking_lot::Mutex::new(None),
        })
    }

    /// Build from a resolved Redis LinkSys, including the apply fingerprint.
    pub(crate) fn from_linksys(name: &str, ls: &LinkSys) -> Result<Arc<Self>> {
        let SystemConfig::Redis(config) = &ls.spec.config else {
            anyhow::bail!("Redis from_linksys called with a non-Redis LinkSys");
        };
        let auth = crate::link_sys::credential::basic_state(ls);
        let fingerprint = super::apply::fingerprint_redis(ls)?;
        let mut client = Self::from_config(name, config, &auth)?;
        client.fingerprint = fingerprint;
        Ok(Arc::new(client))
    }

    pub(crate) fn runtime_fingerprint(&self) -> &RuntimeFingerprint {
        &self.fingerprint
    }

    pub(crate) fn connection_coverage(&self) -> ConnectionCoverage {
        ConnectionCoverage {
            live: self
                .pool
                .clients()
                .iter()
                .filter(|client| client.is_connected())
                .count(),
            configured: self.pool_size,
        }
    }

    pub(crate) fn mark_retired(&self) {
        self.healthy.deactivate();
        self.retired.store(true, Ordering::Relaxed);
        if let Some(shutdown_tx) = self.probe_shutdown.lock().as_ref() {
            let _ = shutdown_tx.send(true);
        }
    }

    /// Initialize connection. Must be called after `from_config()`.
    ///
    /// Connects all pool clients to Redis, records aggregate health, and starts
    /// the background connection-state probe.
    pub async fn init(&self) -> Result<()> {
        self.init_with_resolver(redis_egress_resolver()).await
    }

    /// Initialize against an integration fixture that may be bound to
    /// loopback. Production LinkSys clients always use [`Self::init`].
    #[cfg(feature = "test-util")]
    pub async fn init_for_test(&self) -> Result<()> {
        self.init_with_resolver(redis_egress_resolver_for_test()).await
    }

    async fn init_with_resolver(&self, resolver: Arc<dyn Resolve>) -> Result<()> {
        // Install Edgion's resolver before Fred performs any seed, reconnect,
        // Sentinel, or Cluster topology dial.
        self.pool.set_resolver(resolver).await;

        // Connect all pool clients to Redis
        self.pool
            .init()
            .await
            .map_err(|e| anyhow::anyhow!("Redis [{}] init failed: {:?}", self.name, e))?;

        let connected = pool_has_connected_member(&self.pool);
        self.healthy.update(connected, false);

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let probe_pool = self.pool.clone();
        let probe_retired = self.retired.clone();
        let probe_handle = spawn_health_probe(
            ProbeConfig {
                name: self.name.clone(),
                system_type: "redis",
                base_interval: Duration::from_secs(1),
                max_backoff: Duration::from_secs(1),
                probe_timeout: Duration::from_secs(1),
                upgrade_only: false,
            },
            self.healthy.clone(),
            shutdown_rx,
            move || {
                let pool = probe_pool.clone();
                let retired = probe_retired.clone();
                async move {
                    if retired.load(Ordering::Relaxed) {
                        return false;
                    }
                    let connected = pool_has_connected_member(&pool);
                    if retired.load(Ordering::Relaxed) {
                        return false;
                    }
                    connected
                }
            },
        );
        *self.probe_shutdown.lock() = Some(shutdown_tx);
        *self.probe_handle.lock() = Some(probe_handle);

        tracing::info!(redis = %self.name, "Redis client initialized successfully");
        Ok(())
    }

    /// Check if the client is connected and healthy.
    #[inline]
    pub fn healthy(&self) -> bool {
        self.healthy.load()
    }

    pub(crate) fn published_health(&self) -> &PublishedHealth {
        &self.healthy
    }

    /// Get client name (namespace/name).
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Graceful shutdown — disconnect all pool connections.
    pub async fn shutdown(&self) -> Result<()> {
        if self.shutdown_started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        tracing::info!(redis = %self.name, "Redis client shutting down");
        self.mark_retired();
        let probe_handle = self.probe_handle.lock().take();
        if let Some(probe_handle) = probe_handle {
            let _ = probe_handle.await;
        }

        let quit_result = self
            .pool
            .quit()
            .await
            .map_err(|e| anyhow::anyhow!("Redis [{}] shutdown error: {:?}", self.name, e));
        self.healthy.update(false, false);
        quit_result
    }

    /// Access the underlying fred Pool for advanced operations.
    ///
    /// Prefer using the high-level operations in `ops.rs` when possible.
    /// This is provided as an escape hatch for commands not yet wrapped.
    #[inline]
    pub fn pool(&self) -> &Pool {
        &self.pool
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use edgion_resources::resources::link_sys::redis::RedisPool;

    use super::*;

    #[test]
    fn aggregate_health_uses_any_connected_semantics() {
        assert!(!any_connected([false, false, false]));
        assert!(any_connected([false, true, false]));
        assert!(any_connected([true, true, true]));
    }

    #[test]
    fn uninitialized_pool_has_no_connected_member() {
        let config = RedisClientConfig {
            endpoints: vec!["redis://127.0.0.1:6379".to_string()],
            auth: None,
            db: None,
            timeout: None,
            pool: Some(RedisPool { size: Some(2) }),
            topology: None,
            tls: None,
        };
        let client = RedisLinkClient::from_config("test/redis", &config, &CredentialState::NotRequired).unwrap();

        assert!(!pool_has_connected_member(client.pool()));
        let coverage = client.connection_coverage();
        assert_eq!(coverage.configured, 2);
        assert_eq!(coverage.live, 0);
    }

    #[tokio::test]
    async fn init_installs_egress_resolver_before_fred_connects() {
        let config = RedisClientConfig {
            endpoints: vec!["redis://169.254.169.254:6379".to_string()],
            auth: None,
            db: None,
            timeout: None,
            pool: Some(RedisPool { size: Some(1) }),
            topology: None,
            tls: None,
        };
        let client =
            RedisLinkClient::from_config("test/blocked-redis", &config, &CredentialState::NotRequired).unwrap();

        let error = tokio::time::timeout(Duration::from_secs(1), client.init())
            .await
            .expect("egress rejection must happen before the connection timeout")
            .expect_err("link-local Redis targets must be rejected");

        assert!(
            error
                .to_string()
                .contains("egress policy blocked all resolved addresses"),
            "unexpected initialization error: {error}"
        );
    }
}
