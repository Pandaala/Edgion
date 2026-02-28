//! WebhookManager — global webhook service registry.
//!
//! Manages webhook service connections resolved from LinkSys Webhook resources.
//! Keyed by "namespace/name" matching webhook_ref in KeyGet::Webhook.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use crate::core::plugins::edgion_plugins::common::http_client::{build_webhook_client, get_http_client_arc};
use crate::types::resources::link_sys::webhook::WebhookServiceConfig;

use super::health::spawn_active_health_check;
use super::runtime::{SlidingWindowCounter, WebhookRuntime};

// ============================================================
// Global Manager
// ============================================================

static WEBHOOK_MANAGER: std::sync::LazyLock<WebhookManager> = std::sync::LazyLock::new(WebhookManager::new);

/// Get the global webhook manager
pub fn get_webhook_manager() -> &'static WebhookManager {
    &WEBHOOK_MANAGER
}

/// Global webhook manager, keyed by "namespace/name" matching webhook_ref.
struct WebhookEntry {
    runtime: Arc<WebhookRuntime>,
    health_task: Option<tokio::task::JoinHandle<()>>,
}

pub struct WebhookManager {
    /// Registered webhook services: webhook_ref → runtime state
    services: RwLock<HashMap<String, WebhookEntry>>,
}

impl WebhookManager {
    fn new() -> Self {
        Self {
            services: RwLock::new(HashMap::new()),
        }
    }

    /// Look up a webhook by its ref ("namespace/name")
    pub async fn get(&self, webhook_ref: &str) -> Option<Arc<WebhookRuntime>> {
        self.services.read().await.get(webhook_ref).map(|e| e.runtime.clone())
    }

    /// Register or update a webhook from a LinkSys resource change
    pub async fn upsert(&self, webhook_ref: &str, config: WebhookServiceConfig) {
        let rate_counter = config
            .rate_limit
            .as_ref()
            .map(|rl| SlidingWindowCounter::new(rl.rate, Duration::from_secs(rl.window_sec.max(1))));

        let http_client = match build_webhook_client(&config) {
            None => get_http_client_arc(),
            Some(Ok(client)) => Arc::new(client),
            Some(Err(e)) => {
                tracing::error!(
                    webhook = %webhook_ref,
                    error = %e,
                    "Failed to build webhook HTTP client with TLS config, falling back to default"
                );
                get_http_client_arc()
            }
        };

        let runtime = Arc::new(WebhookRuntime {
            config,
            http_client,
            healthy: AtomicBool::new(true),
            passive_failures: AtomicU32::new(0),
            last_halfopen: AtomicU64::new(0),
            backoff_sec: AtomicU64::new(0),
            rate_counter,
        });

        let health_task = if runtime
            .config
            .health_check
            .as_ref()
            .and_then(|hc| hc.active.as_ref())
            .is_some()
        {
            let ref_clone = webhook_ref.to_string();
            let rt_clone = runtime.clone();
            Some(tokio::spawn(async move {
                spawn_active_health_check(ref_clone, rt_clone).await;
            }))
        } else {
            None
        };

        let mut services = self.services.write().await;
        if let Some(old) = services.remove(webhook_ref) {
            if let Some(handle) = old.health_task {
                handle.abort();
            }
        }
        services.insert(webhook_ref.to_string(), WebhookEntry { runtime, health_task });
        tracing::info!(webhook = %webhook_ref, "Webhook registered/updated in manager");
    }

    /// Remove a webhook (LinkSys resource deleted)
    pub async fn remove(&self, webhook_ref: &str) {
        if let Some(entry) = self.services.write().await.remove(webhook_ref) {
            if let Some(handle) = entry.health_task {
                handle.abort();
            }
        }
        tracing::info!(webhook = %webhook_ref, "Webhook removed from manager");
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[tokio::test]
    async fn test_manager_upsert_and_get() {
        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            uri: "http://localhost:8080/resolve".to_string(),
            ..Default::default()
        };
        manager.upsert("test/webhook", config).await;

        let runtime = manager.get("test/webhook").await;
        assert!(runtime.is_some());
        assert!(runtime.unwrap().healthy.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn test_manager_remove() {
        let manager = WebhookManager::new();
        let config = WebhookServiceConfig {
            uri: "http://localhost:8080/resolve".to_string(),
            ..Default::default()
        };
        manager.upsert("test/webhook", config).await;
        manager.remove("test/webhook").await;

        let runtime = manager.get("test/webhook").await;
        assert!(runtime.is_none());
    }
}
