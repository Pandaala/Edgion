//! RedisLinkClient — runtime Redis client wrapper.
//!
//! Built from LinkSys CRD config, managed by LinkSysStore (ConfHandler-driven).
//! Wraps a `fred::clients::Pool` which internally manages:
//! - Connection pool (round-robin, configurable size)
//! - Automatic reconnection (exponential backoff)
//! - TLS (rustls)
//! - Cluster slot routing / Sentinel failover

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use fred::clients::Pool;
use fred::interfaces::{ClientLike, EventInterface};

use crate::types::resources::link_sys::redis::RedisClientConfig;

use super::config_mapping::{build_fred_config, build_fred_pool};

/// Runtime Redis client wrapper.
///
/// This struct wraps a `fred::clients::Pool` (not a single `Client`) to support
/// concurrent access with internal round-robin connection pooling.
///
/// Lifecycle is managed by `LinkSysStore` via the `ConfHandler` pattern:
/// - CRD created → `from_config()` + `init()` → stored in global store
/// - CRD updated → new client built, old client shut down in background
/// - CRD deleted → `shutdown()` called, removed from store
pub struct RedisLinkClient {
    /// fred connection pool (round-robin across connections)
    pool: Pool,
    /// Human-readable name ("namespace/name")
    name: String,
    /// Atomic health flag, updated by connection event listener
    healthy: Arc<AtomicBool>,
}

impl RedisLinkClient {
    /// Create from CRD config. Does NOT connect — call `init()` next.
    ///
    /// Maps CRD config → fred Config + Builder, then builds a connection pool.
    /// The pool is not yet connected; call `init()` to establish connections.
    pub fn from_config(name: &str, config: &RedisClientConfig) -> Result<Self> {
        let fred_config = build_fred_config(config)?;
        let (builder, pool_size) = build_fred_pool(config, fred_config)?;

        let pool = builder
            .build_pool(pool_size)
            .map_err(|e| anyhow::anyhow!("failed to build Redis pool for {}: {:?}", name, e))?;

        Ok(Self {
            pool,
            name: name.to_string(),
            healthy: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Initialize connection. Must be called after `from_config()`.
    ///
    /// Sets up event listeners on each client in the pool for health tracking,
    /// then connects all pool clients to Redis.
    /// On success, the client is marked healthy and ready for use.
    pub async fn init(&self) -> Result<()> {
        // Set up connection event listeners on each client in the pool.
        // Pool does not implement EventInterface directly — we must iterate
        // over individual clients.
        for client in self.pool.clients() {
            let health = self.healthy.clone();
            let name = self.name.clone();

            // on_reconnect fires on initial connect AND every reconnect
            client.on_reconnect(move |server| {
                let name = name.clone();
                let health = health.clone();
                async move {
                    tracing::info!(redis = %name, server = %server, "Redis (re)connected");
                    health.store(true, Ordering::Relaxed);
                    Ok(())
                }
            });

            let health_err = self.healthy.clone();
            let name_err = self.name.clone();
            client.on_error(move |(error, server)| {
                let name = name_err.clone();
                let health = health_err.clone();
                async move {
                    tracing::warn!(
                        redis = %name,
                        server = ?server,
                        error = %error,
                        "Redis connection error"
                    );
                    health.store(false, Ordering::Relaxed);
                    Ok(())
                }
            });
        }

        // Connect all pool clients to Redis
        self.pool
            .init()
            .await
            .map_err(|e| anyhow::anyhow!("Redis [{}] init failed: {:?}", self.name, e))?;

        self.healthy.store(true, Ordering::Relaxed);
        tracing::info!(redis = %self.name, "Redis client initialized successfully");
        Ok(())
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

    /// Graceful shutdown — disconnect all pool connections.
    pub async fn shutdown(&self) -> Result<()> {
        tracing::info!(redis = %self.name, "Redis client shutting down");
        self.pool
            .quit()
            .await
            .map_err(|e| anyhow::anyhow!("Redis [{}] shutdown error: {:?}", self.name, e))?;
        self.healthy.store(false, Ordering::Relaxed);
        Ok(())
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
