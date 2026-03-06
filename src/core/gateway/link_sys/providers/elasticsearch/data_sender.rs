//! Elasticsearch DataSender — the primary `DataSender<String>` implementation for log shipping.
//!
//! Sends JSON log strings to ES via the background bulk ingest task.
//! When ES is unavailable, falls back to FailedCache (LocalFileWriter or Redis).
//!
//! This is the implementation envisioned in `DataSender` trait's design comments
//! (the "EsSender" referenced in data_sender_trait.rs).

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::core::gateway::link_sys::DataSender;

use super::client::EsLinkClient;

/// Elasticsearch DataSender.
///
/// # FailedCache Pattern
/// When ES is unavailable, logs are sent to the `failed_cache` (LocalFileWriter or Redis).
/// Recovery from FailedCache is a separate background task (future iteration).
pub struct EsDataSender {
    /// ES client reference
    client: Arc<EsLinkClient>,
    /// Fallback sender when ES is unavailable
    failed_cache: Option<Box<dyn DataSender<String>>>,
}

impl EsDataSender {
    /// Create a new EsDataSender.
    pub fn new(client: Arc<EsLinkClient>, failed_cache: Option<Box<dyn DataSender<String>>>) -> Self {
        Self { client, failed_cache }
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
                Ok(()) => return Ok(()),
                Err(e) => {
                    tracing::warn!("ES send failed, falling back to cache: {:?}", e);
                    // Fall through to failed cache
                }
            }
        }

        // ES unavailable — use FailedCache
        if let Some(cache) = &self.failed_cache {
            cache.send(data).await?;
        } else {
            tracing::error!("ES unavailable and no FailedCache configured — log dropped");
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "elasticsearch"
    }
}
