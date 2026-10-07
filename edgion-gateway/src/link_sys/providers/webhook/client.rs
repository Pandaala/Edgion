//! Owned Webhook runtime prepared before publication in the unified LinkSys table.

use std::sync::Arc;

use edgion_common::link_sys_core::outbound_tls::{
    outbound_global_default, validate_active_tls_inheritance, validate_local_tls_selection, GlobalOutboundTlsSnapshot,
    TlsMaterialProofError,
};
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};
use edgion_resources::resources::refs::link_sys_fields::PreparedLinkSysReferences;

use super::apply::fingerprint_webhook;
use crate::link_sys::runtime::apply::RuntimeFingerprint;
use crate::plugins::http::common::outbound::client::ManagedOutboundClient;

/// Immutable actual input shared by comparison and preparation. Private fields
/// prevent detached configuration or material from replacing owning provenance.
pub(crate) struct WebhookCandidate {
    owner: Arc<LinkSys>,
    references: PreparedLinkSysReferences,
    defaults: Arc<GlobalOutboundTlsSnapshot>,
    authority: Result<(), TlsMaterialProofError>,
    fingerprint: Result<RuntimeFingerprint, &'static str>,
}

impl WebhookCandidate {
    pub(crate) fn from_link_sys(owner: Arc<LinkSys>) -> Option<Self> {
        Self::with_snapshot(owner, outbound_global_default())
    }

    pub(crate) fn with_snapshot(owner: Arc<LinkSys>, defaults: Arc<GlobalOutboundTlsSnapshot>) -> Option<Self> {
        if !matches!(&owner.spec.config, SystemConfig::Webhook(_)) {
            return None;
        }
        let references = PreparedLinkSysReferences::from_link_sys(&owner);
        let authority = references
            .bind(&owner)
            .map_err(TlsMaterialProofError::Reference)
            .and_then(|binding| {
                let selection = binding
                    .outbound_tls_selection()
                    .map_err(TlsMaterialProofError::Reference)?;
                let proof = validate_local_tls_selection(selection)?;
                validate_active_tls_inheritance(&proof, &defaults)
            });
        let fingerprint = fingerprint_webhook(&owner, &defaults).map_err(|_| "invalid Webhook fingerprint");
        Some(Self {
            owner,
            references,
            defaults,
            authority,
            fingerprint,
        })
    }

    pub(crate) fn can_reuse(&self, existing: &WebhookLinkClient, forced: bool) -> bool {
        if forced || self.authority.is_err() {
            return false;
        }
        self.fingerprint.as_ref().is_ok_and(|incoming| {
            let live = existing.runtime_fingerprint();
            live.equal_runtime(incoming) || live.rv_skip_allows_reuse(incoming)
        })
    }

    pub(crate) fn prepare(self, key: &str) -> anyhow::Result<WebhookLinkClient> {
        self.authority?;
        let fingerprint = self.fingerprint.map_err(anyhow::Error::msg)?;
        let binding = self.references.bind(&self.owner)?;
        let client = ManagedOutboundClient::from_webhook(&binding, &self.defaults, key)
            .map_err(|error| anyhow::anyhow!("failed to prepare webhook '{key}': {error}"))?;
        Ok(WebhookLinkClient {
            client: Arc::new(client),
            fingerprint,
        })
    }
}

pub struct WebhookLinkClient {
    client: Arc<ManagedOutboundClient>,
    fingerprint: RuntimeFingerprint,
}

impl WebhookLinkClient {
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
