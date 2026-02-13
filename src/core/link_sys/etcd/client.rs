//! EtcdLinkClient — runtime Etcd client wrapper.
//!
//! Built from LinkSys CRD config, managed by LinkSysStore (ConfHandler-driven).
//! Wraps `etcd_client::Client` with:
//! - Auto-reconnect via background health monitor (etcd-client has no built-in reconnect)
//! - Health tracking via AtomicBool
//! - Namespace prefix support for key isolation
//!
//! Unlike fred (Redis), etcd-client uses gRPC multiplexing — a single connection
//! handles multiple concurrent streams, so no connection pool is needed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::RwLock;

use crate::types::resources::link_sys::etcd::EtcdClientConfig;

use super::config_mapping::{build_connect_options, default_connect_options};

/// Runtime Etcd client wrapper.
///
/// Lifecycle is managed by `LinkSysStore` via the `ConfHandler` pattern:
/// - CRD created → `from_config()` + `init()` → stored in global store
/// - CRD updated → new client built, old client shut down in background
/// - CRD deleted → `shutdown()` called, removed from store
pub struct EtcdLinkClient {
    /// etcd client (wrapped in RwLock for reconnect swaps)
    client: Arc<RwLock<Option<etcd_client::Client>>>,
    /// Original config (kept for reconnection)
    config: EtcdClientConfig,
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
    pub fn from_config(name: &str, config: &EtcdClientConfig) -> Result<Self> {
        if config.endpoints.is_empty() {
            anyhow::bail!("Etcd endpoints list is empty for '{}'", name);
        }

        let (shutdown_tx, _) = tokio::sync::watch::channel(false);

        Ok(Self {
            client: Arc::new(RwLock::new(None)),
            config: config.clone(),
            endpoints: config.endpoints.clone(),
            name: name.to_string(),
            healthy: Arc::new(AtomicBool::new(false)),
            namespace: config.namespace.clone(),
            shutdown_tx,
        })
    }

    /// Initialize connection and start background health monitor.
    ///
    /// Sets up the etcd connection and spawns a background task that monitors
    /// connection health and reconnects with exponential backoff on failure.
    pub async fn init(&self) -> Result<()> {
        // Build connect options from CRD config
        let options = build_connect_options(&self.config)?;

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
        tracing::info!(
            etcd = %self.name,
            endpoints = ?self.endpoints,
            "Etcd client initialized successfully"
        );

        // Start background health monitor + reconnect task
        self.start_health_monitor();

        Ok(())
    }

    /// Start background task that monitors health and reconnects on failure.
    fn start_health_monitor(&self) {
        let client = self.client.clone();
        let config = self.config.clone();
        let endpoints = self.endpoints.clone();
        let name = self.name.clone();
        let healthy = self.healthy.clone();
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        tokio::spawn(async move {
            let base_interval = Duration::from_secs(10);
            let mut check_interval = base_interval;
            let mut backoff = Duration::from_millis(500);
            let max_backoff = Duration::from_secs(30);

            loop {
                tokio::select! {
                    _ = tokio::time::sleep(check_interval) => {}
                    _ = shutdown_rx.changed() => {
                        tracing::info!(etcd = %name, "Etcd health monitor shutting down");
                        return;
                    }
                }

                // Try a simple status call as health check
                let is_healthy = {
                    let guard = client.read().await;
                    if let Some(c) = guard.as_ref() {
                        let mut mc = c.clone();
                        mc.status().await.is_ok()
                    } else {
                        false
                    }
                };

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
                    let options = build_connect_options(&config).ok().flatten();
                    let connect_options = options.unwrap_or_else(default_connect_options);
                    match etcd_client::Client::connect(&endpoints, Some(connect_options)).await {
                        Ok(new_client) => {
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
                            check_interval = backoff;
                            backoff = (backoff * 2).min(max_backoff);
                        }
                    }
                }
            }
        });
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

    /// Graceful shutdown — signal health monitor to stop and clear the client.
    pub async fn shutdown(&self) -> Result<()> {
        tracing::info!(etcd = %self.name, "Etcd client shutting down");
        let _ = self.shutdown_tx.send(true);
        let mut guard = self.client.write().await;
        *guard = None;
        self.healthy.store(false, Ordering::Relaxed);
        Ok(())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::link_sys::etcd::*;

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

    #[test]
    fn test_from_config_minimal() {
        let config = minimal_config();
        let client = EtcdLinkClient::from_config("default/etcd-test", &config);
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
        let client = EtcdLinkClient::from_config("default/etcd-ns", &config).unwrap();
        assert_eq!(client.namespace(), Some("/app/"));
        assert_eq!(client.full_key("mykey"), "/app/mykey");
    }

    #[test]
    fn test_from_config_without_namespace() {
        let config = minimal_config();
        let client = EtcdLinkClient::from_config("default/etcd-test", &config).unwrap();
        assert_eq!(client.full_key("mykey"), "mykey");
    }

    #[test]
    fn test_empty_endpoints_returns_error() {
        let config = EtcdClientConfig {
            endpoints: vec![],
            ..minimal_config()
        };
        let result = EtcdLinkClient::from_config("test", &config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }
}
