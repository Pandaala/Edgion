//! Elasticsearch DataSender — the primary `DataSender<String>` implementation for log shipping.
//!
//! Sends JSON log strings to ES via the background bulk ingest task.
//! When ES is unavailable, falls back to FailedCache (LocalFileWriter or Redis).
//!
//! This is the implementation envisioned in `DataSender` trait's design comments
//! (the "EsSender" referenced in data_sender_trait.rs).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::core::gateway::link_sys::DataSender;
use crate::core::gateway::observe::metrics::registry::record_link_sys_es_send;

use super::client::EsLinkClient;

/// Elasticsearch DataSender.
///
/// # FailedCache Pattern
/// When ES is unavailable, logs are sent to the `failed_cache` (LocalFileWriter or Redis).
/// Recovery from FailedCache is a separate background task (future iteration).
///
/// # Logging discipline
/// `send` runs once per access-log record; any per-call `tracing::warn!` / `error!`
/// would double the log volume when ES is unreachable. `es_healthy` tracks health
/// transitions so a single `info!` is emitted on the first failure and on recovery;
/// steady-state success and steady-state failure are metric-only.
/// The "no FailedCache configured" warning is also emitted exactly once at
/// construction time instead of per-send.
pub struct EsDataSender {
    /// ES client reference
    client: Arc<EsLinkClient>,
    /// Fallback sender when ES is unavailable
    failed_cache: Option<Box<dyn DataSender<String>>>,
    /// `true` while ES is currently shipping successfully. Flipped on the first
    /// send error and flipped back on the first recovery; used to dedupe
    /// `tracing::info!` so operators get one transition notice, not N per second.
    es_healthy: AtomicBool,
}

impl EsDataSender {
    /// Create a new EsDataSender.
    pub fn new(client: Arc<EsLinkClient>, failed_cache: Option<Box<dyn DataSender<String>>>) -> Self {
        if failed_cache.is_none() {
            // Emitted once at construction instead of per `send()` call.
            tracing::warn!(
                "EsDataSender: no FailedCache configured — if ES becomes unreachable, access logs will be dropped"
            );
        }
        Self {
            client,
            failed_cache,
            es_healthy: AtomicBool::new(true),
        }
    }
}

#[async_trait]
impl DataSender<String> for EsDataSender {
    async fn init(&mut self) -> Result<()> {
        // Client should already be initialized via EsLinkClient::init()
        // Initialize failed cache if present
        if let Some(cache) = &mut self.failed_cache {
            cache.init().await?;
        }
        Ok(())
    }

    fn healthy(&self) -> bool {
        // EsDataSender is healthy if either ES or FailedCache is available
        self.client.healthy() || self.failed_cache.as_ref().map(|c| c.healthy()).unwrap_or(false)
    }

    async fn send(&self, data: String) -> Result<()> {
        if self.client.healthy() {
            // Try sending to ES
            match self.client.send_bulk(data.clone()).await {
                Ok(()) => {
                    // Recovery transition: flip back to healthy and announce once.
                    if self
                        .es_healthy
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        tracing::info!("ES recovered, resuming direct ingest");
                    }
                    record_link_sys_es_send("ok");
                    return Ok(());
                }
                Err(e) => {
                    // Failure transition: flip to failed and announce once.
                    if self
                        .es_healthy
                        .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        tracing::info!(error = %e, "ES unavailable, falling back to cache");
                    }
                    // Fall through to failed cache
                }
            }
        }

        // ES unavailable — use FailedCache
        if let Some(cache) = &self.failed_cache {
            cache.send(data).await?;
            record_link_sys_es_send("failed_fallback");
        } else {
            // Per-record warn removed — construction-time warn already flagged this.
            // Metric still increments so operators can alert on drops.
            record_link_sys_es_send("failed_no_cache");
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "elasticsearch"
    }
}
