//! Owned Webhook runtime prepared before publication in the unified LinkSys table.

use std::sync::Arc;

use edgion_resources::resources::link_sys::webhook::WebhookServiceConfig;

use crate::link_sys::runtime::apply::RuntimeFingerprint;
use crate::plugins::http::common::outbound::client::ManagedOutboundClient;

pub struct WebhookLinkClient {
    client: Arc<ManagedOutboundClient>,
    fingerprint: RuntimeFingerprint,
}

impl WebhookLinkClient {
    pub(crate) fn prepare(
        key: &str,
        mut config: WebhookServiceConfig,
        namespace: Option<&str>,
        fingerprint: RuntimeFingerprint,
    ) -> anyhow::Result<Self> {
        // TLS block presence selects the shared outbound transport. Disabled
        // LinkSys TLS policy must not reach that engine as an active block.
        config.conn.tls = config.conn.tls.take().filter(|tls| tls.enabled);
        if config.conn.target.url.is_none() && config.conn.target.service.namespace.is_none() {
            if let Some(namespace) = namespace {
                config.conn.target.service.namespace = Some(namespace.to_string());
            }
        }
        let client = ManagedOutboundClient::from_parts(
            &config.conn,
            &config.request,
            config.resolved_secrets.as_ref(),
            key,
            false,
        )
        .map_err(|error| anyhow::anyhow!("failed to prepare webhook '{key}': {error}"))?;
        Ok(Self {
            client: Arc::new(client),
            fingerprint,
        })
    }

    pub(crate) fn managed_client(&self) -> Arc<ManagedOutboundClient> {
        self.client.clone()
    }

    pub(crate) fn runtime_fingerprint(&self) -> &RuntimeFingerprint {
        &self.fingerprint
    }

    /// Stop provider-owned background work; existing call handles may finish.
    pub(crate) fn retire(&self) {
        self.client.stop_health_check();
    }

    #[cfg(test)]
    pub(crate) fn from_managed(client: Arc<ManagedOutboundClient>) -> Self {
        Self {
            client,
            fingerprint: RuntimeFingerprint::unspecified(),
        }
    }
}

impl Drop for WebhookLinkClient {
    fn drop(&mut self) {
        self.retire();
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/webhook/client_test.rs"]
mod tests;
