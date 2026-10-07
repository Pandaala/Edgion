//! Actual Redis ownership and one inherited TLS generation across replay/retry.

use std::sync::Arc;

use super::{outbound_global_default, GlobalOutboundTlsSnapshot, PreparedLinkSysTls};
use crate::link_sys_core::fingerprint::{hash_tls_inheritance, FingerprintHasher};
use edgion_resources::resources::common::OutboundTlsLocal;
use edgion_resources::resources::link_sys::redis::RedisTransport;
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};

/// Immutable input to Redis comparison and construction. Transport enablement
/// comes only from the actual Redis config; inactive local references are inert.
/// ```compile_fail
/// use edgion_common::link_sys_core::outbound_tls::PreparedRedisTls;
/// let prepared = PreparedRedisTls {};
/// ```
pub struct PreparedRedisTls {
    input: PreparedLinkSysTls,
    uses_tls: bool,
}

impl PreparedRedisTls {
    pub fn from_link_sys(owner: Arc<LinkSys>) -> anyhow::Result<Self> {
        Self::with_snapshot(owner, outbound_global_default())
    }

    pub fn with_snapshot(owner: Arc<LinkSys>, defaults: Arc<GlobalOutboundTlsSnapshot>) -> anyhow::Result<Self> {
        let input = PreparedLinkSysTls::with_snapshot(owner, defaults)?;
        let SystemConfig::Redis(config) = &input.owner().spec.config else {
            anyhow::bail!("Redis TLS preparation requires an actual Redis LinkSys");
        };
        let endpoints = config
            .parsed_active_endpoints()
            .map_err(|errors| anyhow::anyhow!(errors.join("; ")))?;
        let uses_tls = config.tls.as_ref().is_some_and(|tls| tls.enabled)
            || endpoints
                .first()
                .is_some_and(|endpoint| endpoint.transport == RedisTransport::Tls);
        if uses_tls {
            input.with_tls_selection(|proof, defaults| {
                let empty = OutboundTlsLocal::default();
                let policy = proof.active_field().map_or(&empty, |field| field.policy());
                for axis in defaults.effective_proofs(policy) {
                    axis?;
                }
                Ok(())
            })?;
        }
        Ok(Self { input, uses_tls })
    }

    pub fn owner(&self) -> &LinkSys {
        self.input.owner()
    }

    pub fn uses_tls(&self) -> bool {
        self.uses_tls
    }

    /// Pure identity for consumed inherited axes; never resolves PEM or audits.
    pub fn hash_inheritance(&self, hasher: &mut FingerprintHasher) {
        let SystemConfig::Redis(config) = &self.input.owner().spec.config else {
            unreachable!("prepared Redis provider is immutable")
        };
        let empty = OutboundTlsLocal::default();
        let policy = self.uses_tls.then(|| {
            config
                .tls
                .as_ref()
                .filter(|tls| tls.enabled)
                .map_or(&empty, |tls| &tls.outbound)
        });
        hash_tls_inheritance(hasher, policy, self.input.defaults());
    }

    /// Cold construction against the same snapshot used before reuse. Fred owns
    /// the resulting connector for seed, retry and reconnect dials.
    pub fn rustls_client_config(&self) -> anyhow::Result<Option<rustls::ClientConfig>> {
        if !self.uses_tls {
            return Ok(None);
        }
        self.input.with_tls_selection(|proof, defaults| {
            crate::link_sys_core::tls::build_validated_rustls_client_config(proof, defaults, "Redis")
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/link_sys_core/outbound_tls/redis_test.rs"]
mod tests;
