//! EsLinkClient — runtime Elasticsearch client wrapper.
//!
//! Built from LinkSys CRD config, managed by LinkSysStore (ConfHandler-driven).
//! Wraps a `reqwest::Client` which internally manages:
//! - Connection pool (keep-alive, configurable max idle)
//! - TLS (rustls, via reqwest)
//! - Default auth headers
//!
//! Bulk ingest runs in a background task with buffer + flush logic.
//!
//! Unlike Redis/Etcd, ES uses plain HTTP REST — reqwest provides built-in
//! connection pooling and keep-alive, so no additional pool management is needed.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use reqwest::Client;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use async_trait::async_trait;

use crate::link_sys::credential::{CredentialState, ResolvedCredential};
use crate::link_sys::lifecycle::LinkSysLifecycle;
use edgion_resources::resources::link_sys::elasticsearch::ElasticsearchClientConfig;

use super::bulk::bulk_ingest_loop;
use super::config_mapping::{build_es_client, EsBulkConfig};

/// Runtime Elasticsearch client wrapper.
///
/// Lifecycle is managed by `LinkSysStore` via the `ConfHandler` pattern:
/// - CRD created → `from_config()` + `init()` → stored in global store
/// - CRD updated → new client built, old client shut down in background
/// - CRD deleted → `shutdown()` called, removed from store
pub struct EsLinkClient {
    /// reqwest HTTP client (with internal pool and auth headers)
    client: Client,
    /// Elasticsearch endpoints (round-robin)
    endpoints: Vec<String>,
    /// Current endpoint index for round-robin
    endpoint_index: AtomicUsize,
    /// Human-readable name ("namespace/name")
    name: String,
    /// Atomic health flag, updated by health check and bulk operations
    healthy: Arc<AtomicBool>,
    /// Bulk ingest configuration
    bulk_config: EsBulkConfig,
    /// Channel sender for async bulk batching (populated after init)
    bulk_sender: parking_lot::Mutex<Option<mpsc::Sender<String>>>,
    /// Shutdown signal sender (populated after init)
    shutdown_tx: parking_lot::Mutex<Option<watch::Sender<bool>>>,
    /// Handle to the spawned bulk ingest task (populated after init), awaited in `shutdown()`
    /// to drain buffered docs before returning.
    bulk_task: parking_lot::Mutex<Option<JoinHandle<()>>>,
}

impl EsLinkClient {
    /// Create from CRD config. Does NOT connect — call `init()` next.
    pub fn from_config(
        name: &str,
        config: &ElasticsearchClientConfig,
        auth: &CredentialState<ResolvedCredential>,
    ) -> Result<Self> {
        if config.endpoints.is_empty() {
            anyhow::bail!("Elasticsearch endpoints list is empty for '{}'", name);
        }

        let (client, _headers) = build_es_client(config, auth)?;
        let bulk_config = EsBulkConfig::from_crd(config);

        Ok(Self {
            client,
            endpoints: config.endpoints.clone(),
            endpoint_index: AtomicUsize::new(0),
            name: name.to_string(),
            healthy: Arc::new(AtomicBool::new(false)),
            bulk_config,
            bulk_sender: parking_lot::Mutex::new(None),
            shutdown_tx: parking_lot::Mutex::new(None),
            bulk_task: parking_lot::Mutex::new(None),
        })
    }

    /// Initialize connection. Verifies connectivity via cluster health API,
    /// then spawns the background bulk ingest task.
    pub async fn init(&self) -> Result<()> {
        // Verify connectivity with a health check
        let health = self.cluster_health().await;
        match &health {
            Ok(status) => {
                tracing::info!("ES [{}] connected, cluster status: {}", self.name, status);
            }
            Err(e) => {
                tracing::warn!(
                    "ES [{}] initial health check failed: {:?} — will retry in background",
                    self.name,
                    e
                );
                // Don't fail init — ES may become available later
            }
        }

        self.healthy.store(health.is_ok(), Ordering::Relaxed);

        // Spawn background bulk ingest task. Channel capacity is operator-tunable
        // via `bulk.channelSize` (defaults to 10k, ceiling 100k); see EsBulkConfig.
        let (tx, rx) = mpsc::channel::<String>(self.bulk_config.channel_size);
        let (shutdown_sender, shutdown_rx) = watch::channel(false);

        let client = self.client.clone();
        let endpoints = self.endpoints.clone();
        let bulk_config = self.bulk_config.clone();
        let healthy = self.healthy.clone();
        let name = self.name.clone();

        let handle = tokio::spawn(async move {
            bulk_ingest_loop(client, endpoints, bulk_config, healthy, name, rx, shutdown_rx).await;
        });

        *self.bulk_sender.lock() = Some(tx);
        *self.shutdown_tx.lock() = Some(shutdown_sender);
        *self.bulk_task.lock() = Some(handle);

        tracing::info!("ES [{}] initialized, bulk ingest task started", self.name);
        Ok(())
    }

    /// Get the next endpoint (round-robin).
    pub fn next_endpoint(&self) -> &str {
        if self.endpoints.is_empty() {
            return "";
        }
        let idx = self.endpoint_index.fetch_add(1, Ordering::Relaxed) % self.endpoints.len();
        &self.endpoints[idx]
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

    /// Get the configured endpoints.
    pub fn endpoints(&self) -> &[String] {
        &self.endpoints
    }

    /// Get the bulk configuration.
    pub fn bulk_config(&self) -> &EsBulkConfig {
        &self.bulk_config
    }

    /// Send a document to the bulk ingest buffer.
    ///
    /// Tries a non-blocking send first; on `Full`, increments the
    /// `link_sys_es_bulk_channel_full` counter and falls back to an awaiting
    /// send so the producer backpressures rather than drops the document. On
    /// `Closed` (receiver gone), returns a "bulk channel closed" error so the
    /// caller can decide how to handle the drop.
    pub async fn send_bulk(&self, doc: String) -> Result<()> {
        let sender = self.bulk_sender.lock().clone();
        let Some(sender) = sender else {
            anyhow::bail!("ES [{}] not initialized (bulk sender not ready)", self.name);
        };
        match sender.try_send(doc) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(doc)) => {
                crate::observe::metrics::registry::record_link_sys_es_bulk_channel_full();
                sender
                    .send(doc)
                    .await
                    .map_err(|_| anyhow::anyhow!("ES [{}] bulk channel closed", self.name))?;
                Ok(())
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Err(anyhow::anyhow!("ES [{}] bulk channel closed", self.name)),
        }
    }

    /// Graceful shutdown: signal background task and drop sender, then await drain.
    pub async fn shutdown(&self) -> Result<()> {
        tracing::info!("ES [{}] shutting down", self.name);

        // Signal shutdown to background task
        if let Some(shutdown) = self.shutdown_tx.lock().take() {
            let _ = shutdown.send(true);
        }

        // Drop the bulk sender to unblock the receiver
        *self.bulk_sender.lock() = None;
        self.healthy.store(false, Ordering::Relaxed);

        // Await the bulk task so buffered docs get flushed before we return.
        // Bound the wait so a hung HTTP flush cannot stall the caller indefinitely
        // (per-request timeout + retries can reach ~90s; we cap the total wait at 10s).
        let handle = self.bulk_task.lock().take();
        if let Some(handle) = handle {
            // Capture an abort handle before moving the join handle into `timeout`.
            // `tokio::time::timeout` over a JoinHandle only stops the caller from
            // waiting — it does NOT abort the spawned task. Without an explicit
            // abort, a task stuck in an in-flight HTTP request keeps running
            // detached past the 10s budget. `bulk_ingest_loop` already exits its
            // retry loop promptly on the shutdown signal; this abort is the hard
            // backstop for a single request hung beyond the budget.
            let abort_handle = handle.abort_handle();
            match tokio::time::timeout(Duration::from_secs(10), handle).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!("ES [{}] bulk task join error: {:?}", self.name, e);
                }
                Err(_) => {
                    abort_handle.abort();
                    tracing::warn!(
                        "ES [{}] bulk task did not drain within 10s; aborting (buffered docs may be lost)",
                        self.name
                    );
                }
            }
        }

        Ok(())
    }

    /// Access the underlying reqwest Client for custom requests.
    #[inline]
    pub fn inner(&self) -> &Client {
        &self.client
    }

    /// Set the healthy flag (used by health check operations).
    pub(crate) fn set_healthy(&self, val: bool) {
        self.healthy.store(val, Ordering::Relaxed);
    }
}

impl std::fmt::Debug for EsLinkClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EsLinkClient")
            .field("name", &self.name)
            .field("endpoints", &self.endpoints)
            .field("healthy", &self.healthy.load(Ordering::Relaxed))
            .finish()
    }
}

#[async_trait]
impl LinkSysLifecycle for EsLinkClient {
    async fn init(&self) -> anyhow::Result<()> {
        EsLinkClient::init(self).await
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        EsLinkClient::shutdown(self).await
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_endpoints(endpoints: Vec<String>) -> ElasticsearchClientConfig {
        ElasticsearchClientConfig {
            endpoints,
            auth: None,
            tls: None,
            timeout: None,
            pool: None,
            bulk: None,
            index: None,
            failed_cache: None,
            compatibility: None,
        }
    }

    fn no_auth() -> CredentialState<ResolvedCredential> {
        CredentialState::NotRequired
    }

    #[test]
    fn test_empty_endpoints_returns_error() {
        let config = config_with_endpoints(vec![]);
        let result = EsLinkClient::from_config("test", &config, &no_auth());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }
}
