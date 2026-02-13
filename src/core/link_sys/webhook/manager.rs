//! WebhookManager — global webhook service registry.
//!
//! Manages webhook service connections resolved from LinkSys Webhook resources.
//! Keyed by "namespace/name" matching webhook_ref in KeyGet::Webhook.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use crate::types::resources::link_sys::webhook::WebhookServiceConfig;

use super::health::spawn_active_health_check;
use super::runtime::{SlidingWindowCounter, WebhookRuntime};

// ============================================================
// Global Manager
// ============================================================

static WEBHOOK_MANAGER: std::sync::LazyLock<WebhookManager> =
    std::sync::LazyLock::new(WebhookManager::new);

/// Get the global webhook manager
pub fn get_webhook_manager() -> &'static WebhookManager {
    &WEBHOOK_MANAGER
}

/// Global webhook manager, keyed by "namespace/name" matching webhook_ref.
pub struct WebhookManager {
    /// Registered webhook services: webhook_ref → runtime state
    services: RwLock<HashMap<String, Arc<WebhookRuntime>>>,
}

impl WebhookManager {
    fn new() -> Self {
        Self {
            services: RwLock::new(HashMap::new()),
        }
    }

    /// Look up a webhook by its ref ("namespace/name")
    pub async fn get(&self, webhook_ref: &str) -> Option<Arc<WebhookRuntime>> {
        self.services.read().await.get(webhook_ref).cloned()
    }

    /// Register or update a webhook from a LinkSys resource change
    pub async fn upsert(&self, webhook_ref: &str, config: WebhookServiceConfig) {
        let rate_counter = config.rate_limit.as_ref().map(|rl| {
            SlidingWindowCounter::new(rl.rate, Duration::from_secs(rl.window_sec.max(1)))
        });

        let runtime = Arc::new(WebhookRuntime {
            config,
            healthy: AtomicBool::new(true),
            passive_failures: AtomicU32::new(0),
            last_halfopen: AtomicU64::new(0),
            backoff_sec: AtomicU64::new(0),
            rate_counter,
        });

        // Spawn active health check if configured
        if runtime
            .config
            .health_check
            .as_ref()
            .and_then(|hc| hc.active.as_ref())
            .is_some()
        {
            let ref_clone = webhook_ref.to_string();
            let rt_clone = runtime.clone();
            tokio::spawn(async move {
                spawn_active_health_check(ref_clone, rt_clone).await;
            });
        }

        self.services
            .write()
            .await
            .insert(webhook_ref.to_string(), runtime);
        tracing::info!(webhook = %webhook_ref, "Webhook registered/updated in manager");
    }

    /// Remove a webhook (LinkSys resource deleted)
    pub async fn remove(&self, webhook_ref: &str) {
        self.services.write().await.remove(webhook_ref);
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
