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

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use reqwest::Client;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::link_sys::credential::{CredentialState, ResolvedCredential};
use crate::link_sys::runtime::apply::{ConnectionCoverage, RuntimeFingerprint};
use edgion_resources::resources::link_sys::elasticsearch::ElasticsearchClientConfig;
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};

use super::bulk::bulk_ingest_loop;
use super::config_mapping::{build_es_client, EsBulkConfig};

/// Runtime Elasticsearch client wrapper.
///
/// One published generation. The runner may hold a second retiring Arc during
/// Handover; callers always go through `get_es_client`.
pub struct EsLinkClient {
    /// reqwest HTTP client (with internal pool and auth headers)
    client: Client,
    /// Elasticsearch endpoints (round-robin)
    endpoints: Vec<String>,
    /// Current endpoint index for round-robin
    endpoint_index: AtomicUsize,
    /// Human-readable name ("namespace/name")
    name: String,
    /// Atomic health flag driven by the background probe (upgrade-only recovery)
    /// and the bulk path (both directions); also written at init().
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
    /// Shared flip-log rate-limit clock for the bulk path (CAS emission dedup).
    last_flip_log_ms: Arc<AtomicU64>,
    /// Handle to the background health probe; aborted in `shutdown()`.
    probe_handle: parking_lot::Mutex<Option<JoinHandle<()>>>,
    /// Apply fingerprint captured at construction (D19).
    fingerprint: RuntimeFingerprint,
    /// Set before handover retire so the probe stops writing the shared gauge.
    retired: Arc<AtomicBool>,
    shutdown_started: AtomicBool,
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
            last_flip_log_ms: Arc::new(AtomicU64::new(0)),
            probe_handle: parking_lot::Mutex::new(None),
            fingerprint: RuntimeFingerprint::unspecified(),
            retired: Arc::new(AtomicBool::new(false)),
            shutdown_started: AtomicBool::new(false),
        })
    }

    /// Build from a resolved Elasticsearch LinkSys, including the apply fingerprint.
    pub(crate) fn from_linksys(name: &str, ls: &LinkSys) -> Result<Arc<Self>> {
        let SystemConfig::Elasticsearch(config) = &ls.spec.config else {
            anyhow::bail!("Elasticsearch from_linksys called with a non-Elasticsearch LinkSys");
        };
        let auth = crate::link_sys::credential::resolved_state(ls);
        let fingerprint = super::apply::fingerprint_es(ls)?;
        let mut client = Self::from_config(name, config, &auth)?;
        client.fingerprint = fingerprint;
        Ok(Arc::new(client))
    }

    pub(crate) fn runtime_fingerprint(&self) -> &RuntimeFingerprint {
        &self.fingerprint
    }

    pub(crate) fn connection_coverage(&self) -> ConnectionCoverage {
        // reqwest does not expose pool membership. Generation health is the
        // coverage signal: a healthy cluster is treated as fully covered so a
        // rebuild hands over; an unhealthy generation is ReplaceNow.
        ConnectionCoverage {
            live: if self.healthy() { self.endpoints.len() } else { 0 },
            configured: self.endpoints.len().max(1),
        }
    }

    pub(crate) fn mark_retired(&self) {
        self.retired.store(true, Ordering::Relaxed);
        if let Some(handle) = self.probe_handle.lock().take() {
            handle.abort();
        }
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
        // Emit the baseline health gauge so the series appears even if the
        // provider is healthy from startup and never flips (matches Kafka init).
        crate::observe::metrics::registry::record_link_sys_health(&self.name, health.is_ok());

        // Spawn background bulk ingest task. Channel capacity is operator-tunable
        // via `bulk.channelSize` (defaults to 10k, ceiling 100k); see EsBulkConfig.
        let (tx, rx) = mpsc::channel::<String>(self.bulk_config.channel_size);
        let (shutdown_sender, shutdown_rx) = watch::channel(false);

        let client = self.client.clone();
        let endpoints = self.endpoints.clone();
        let bulk_config = self.bulk_config.clone();
        let healthy = self.healthy.clone();
        let last_flip_log = self.last_flip_log_ms.clone();
        let name = self.name.clone();
        let retired = self.retired.clone();

        let handle = tokio::spawn(async move {
            bulk_ingest_loop(
                client,
                endpoints,
                bulk_config,
                healthy,
                last_flip_log,
                name,
                rx,
                shutdown_rx,
                retired,
            )
            .await;
        });

        // Background health probe: actively RECOVER `healthy` during
        // idle (upgrade_only). The bulk path owns downgrade. Rotates across all
        // endpoints using a probe-local index (NOT the shared round-robin), so a
        // multi-endpoint ES recovers as soon as ANY endpoint is reachable — never
        // stuck false because endpoints[0] alone is down. Upgrade-only, so
        // probing a down endpoint never spuriously downgrades.
        let probe_handle = crate::link_sys::runtime::health_probe::spawn_health_probe(
            crate::link_sys::runtime::health_probe::ProbeConfig {
                name: self.name.clone(),
                system_type: "elasticsearch",
                base_interval: Duration::from_secs(10),
                max_backoff: Duration::from_secs(30),
                probe_timeout: Duration::from_secs(5),
                upgrade_only: true,
            },
            self.healthy.clone(),
            shutdown_sender.subscribe(),
            {
                let client = self.client.clone();
                let endpoints = self.endpoints.clone();
                let probe_idx = Arc::new(AtomicUsize::new(0));
                let retired = self.retired.clone();
                move || {
                    let client = client.clone();
                    let endpoints = endpoints.clone();
                    let probe_idx = probe_idx.clone();
                    let retired = retired.clone();
                    async move {
                        if retired.load(Ordering::Relaxed) {
                            return false;
                        }
                        let i = probe_idx.fetch_add(1, Ordering::Relaxed) % endpoints.len();
                        super::ops::probe_cluster_health(&client, &endpoints[i]).await
                    }
                }
            },
        );

        *self.bulk_sender.lock() = Some(tx);
        *self.shutdown_tx.lock() = Some(shutdown_sender);
        *self.bulk_task.lock() = Some(handle);
        *self.probe_handle.lock() = Some(probe_handle);

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

    /// Non-blocking send for access-log callers.
    ///
    /// Returns Err on Full or Closed; never awaits. Use send_bulk() when
    /// backpressure is preferable to dropping.
    pub fn try_send_bulk(&self, doc: String) -> Result<()> {
        let sender = self.bulk_sender.lock().clone();
        let Some(sender) = sender else {
            anyhow::bail!("ES [{}] not initialized (bulk sender not ready)", self.name);
        };
        sender
            .try_send(doc)
            .map_err(|e| anyhow::anyhow!("ES [{}] try_send_bulk: {}", self.name, e))
    }

    /// Graceful shutdown: signal background task and drop sender, then await drain.
    pub async fn shutdown(&self) -> Result<()> {
        if self.shutdown_started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        tracing::info!("ES [{}] shutting down", self.name);
        self.mark_retired();

        // Signal shutdown to background task
        if let Some(shutdown) = self.shutdown_tx.lock().take() {
            let _ = shutdown.send(true);
        }

        // Drop the bulk sender to unblock the receiver
        *self.bulk_sender.lock() = None;

        // Abort the probe before clearing healthy. abort() is not synchronous;
        // an in-flight probe request may still complete and briefly flip healthy
        // back to true before the task is descheduled, but the client is removed
        // from the store before any caller can observe this stale state.
        if let Some(handle) = self.probe_handle.lock().take() {
            handle.abort();
        }

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

    #[test]
    fn test_try_send_bulk_not_initialized_returns_err() {
        let config = config_with_endpoints(vec!["http://localhost:9200".to_string()]);
        let client = EsLinkClient::from_config("test/client", &config, &no_auth())
            .expect("from_config should succeed with a valid endpoint");

        // bulk_sender is None until init() is called
        let result = client.try_send_bulk("test doc".to_string());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("not initialized"), "unexpected error: {}", msg);
    }
}
