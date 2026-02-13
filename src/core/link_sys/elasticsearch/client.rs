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

use anyhow::Result;
use reqwest::Client;
use tokio::sync::{mpsc, watch};

use crate::types::resources::link_sys::elasticsearch::ElasticsearchClientConfig;

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
}

impl EsLinkClient {
    /// Create from CRD config. Does NOT connect — call `init()` next.
    pub fn from_config(name: &str, config: &ElasticsearchClientConfig) -> Result<Self> {
        let (client, _headers) = build_es_client(config)?;
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

        // Spawn background bulk ingest task
        let (tx, rx) = mpsc::channel::<String>(10_000); // Buffer up to 10k pending docs
        let (shutdown_sender, shutdown_rx) = watch::channel(false);

        let client = self.client.clone();
        let endpoints = self.endpoints.clone();
        let bulk_config = self.bulk_config.clone();
        let healthy = self.healthy.clone();
        let name = self.name.clone();

        tokio::spawn(async move {
            bulk_ingest_loop(client, endpoints, bulk_config, healthy, name, rx, shutdown_rx).await;
        });

        *self.bulk_sender.lock() = Some(tx);
        *self.shutdown_tx.lock() = Some(shutdown_sender);

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
    /// Non-blocking: returns immediately, doc is batched in background.
    pub async fn send_bulk(&self, doc: String) -> Result<()> {
        let sender = self.bulk_sender.lock().clone();
        if let Some(sender) = sender {
            sender
                .send(doc)
                .await
                .map_err(|_| anyhow::anyhow!("ES [{}] bulk channel closed", self.name))?;
        } else {
            anyhow::bail!("ES [{}] not initialized (bulk sender not ready)", self.name);
        }
        Ok(())
    }

    /// Graceful shutdown: signal background task and drop sender.
    pub async fn shutdown(&self) -> Result<()> {
        tracing::info!("ES [{}] shutting down", self.name);

        // Signal shutdown to background task
        if let Some(shutdown) = self.shutdown_tx.lock().take() {
            let _ = shutdown.send(true);
        }

        // Drop the bulk sender to unblock the receiver
        *self.bulk_sender.lock() = None;
        self.healthy.store(false, Ordering::Relaxed);

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
