//! WebhookLinkClient — `LinkSysLifecycle` wrapper around the global `WebhookManager`.
//!
//! Holds the per-LinkSys-resource view of a webhook (key + config). The actual
//! manager state lives in the global `WebhookManager`; this struct's `init` /
//! `shutdown` proxy through `WebhookManagerOps` so production wires to the
//! global manager via `StaticWebhookManagerAdapter` while tests inject a fake.
//!
//! `SHUTDOWN_ON_REPLACE = false` because `init` is an idempotent
//! `manager.upsert`. Calling `shutdown` (i.e. `manager.remove`) on the old
//! instance after an update would delete the entry just written by the new
//! instance's `init`.

use std::sync::Arc;

use async_trait::async_trait;

use crate::link_sys::lifecycle::LinkSysLifecycle;
use crate::link_sys::providers::webhook::manager::{StaticWebhookManagerAdapter, WebhookManagerOps};
use edgion_resources::resources::link_sys::webhook::WebhookServiceConfig;

pub struct WebhookLinkClient {
    key: String,
    config: WebhookServiceConfig,
    linksys_ns: Option<String>,
    manager: Arc<dyn WebhookManagerOps>,
}

impl WebhookLinkClient {
    /// Production constructor — wires to the global `WebhookManager` via
    /// the zero-sized `StaticWebhookManagerAdapter`.
    pub fn new(key: impl Into<String>, config: WebhookServiceConfig, linksys_ns: Option<String>) -> Self {
        Self::with_manager(
            key,
            config,
            linksys_ns,
            Arc::new(StaticWebhookManagerAdapter) as Arc<dyn WebhookManagerOps>,
        )
    }

    /// Test constructor — accepts any `WebhookManagerOps` implementor.
    pub fn with_manager(
        key: impl Into<String>,
        config: WebhookServiceConfig,
        linksys_ns: Option<String>,
        manager: Arc<dyn WebhookManagerOps>,
    ) -> Self {
        Self {
            key: key.into(),
            config,
            linksys_ns,
            manager,
        }
    }
}

#[async_trait]
impl LinkSysLifecycle for WebhookLinkClient {
    const SHUTDOWN_ON_REPLACE: bool = false;

    async fn init(&self) -> anyhow::Result<()> {
        self.manager
            .upsert(&self.key, self.config.clone(), self.linksys_ns.as_deref())
            .await
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        self.manager.remove(&self.key).await;
        Ok(())
    }
}

// Compile-time assertion documenting Webhook's idempotent-upsert semantic.
const _ASSERT_NO_SHUTDOWN_ON_REPLACE: () = {
    assert!(!<WebhookLinkClient as LinkSysLifecycle>::SHUTDOWN_ON_REPLACE);
};

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    #[derive(Default)]
    struct FakeManager {
        calls: Mutex<Vec<(String, String)>>, // (op, key)
    }

    #[async_trait]
    impl WebhookManagerOps for FakeManager {
        async fn upsert(
            &self,
            key: &str,
            _config: WebhookServiceConfig,
            _linksys_ns: Option<&str>,
        ) -> anyhow::Result<()> {
            self.calls.lock().push(("upsert".into(), key.into()));
            Ok(())
        }

        async fn remove(&self, key: &str) {
            self.calls.lock().push(("remove".into(), key.into()));
        }
    }

    fn config() -> WebhookServiceConfig {
        use edgion_resources::resources::link_sys::webhook::WebhookTarget;
        WebhookServiceConfig {
            target: WebhookTarget::Uri {
                uri: "http://example/x".into(),
            },
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn init_calls_upsert_on_manager() {
        let mgr = Arc::new(FakeManager::default());
        let client =
            WebhookLinkClient::with_manager("ns/wh", config(), None, mgr.clone() as Arc<dyn WebhookManagerOps>);

        client.init().await.unwrap();

        let calls = mgr.calls.lock().clone();
        assert_eq!(calls, vec![("upsert".to_string(), "ns/wh".to_string())]);
    }

    #[tokio::test]
    async fn shutdown_calls_remove_on_manager() {
        let mgr = Arc::new(FakeManager::default());
        let client =
            WebhookLinkClient::with_manager("ns/wh", config(), None, mgr.clone() as Arc<dyn WebhookManagerOps>);

        client.shutdown().await.unwrap();

        let calls = mgr.calls.lock().clone();
        assert_eq!(calls, vec![("remove".to_string(), "ns/wh".to_string())]);
    }
}
