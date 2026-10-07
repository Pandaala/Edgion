//! Actual OTLP ownership and captured HTTPS defaults before reuse or dialing.
use std::sync::Arc;

use super::{
    outbound_global_default, resolve_validated_outbound_tls_audited, GlobalOutboundTlsSnapshot, PreparedLinkSysTls,
    ResolvedOutboundTls,
};
use crate::link_sys_core::fingerprint::{hash_tls_inheritance, FingerprintHasher};
use edgion_resources::resources::common::OutboundTlsLocal;
use edgion_resources::resources::link_sys::otlp::OtlpEndpoint;
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig};

/// Immutable actual OTLP source, reference topology and inherited generation.
/// HTTPS uses inherited TLS even when local TLS is absent or disabled.
/// ```compile_fail
/// use edgion_common::link_sys_core::outbound_tls::PreparedOtlpTls;
/// let prepared = PreparedOtlpTls {};
/// ```
pub struct PreparedOtlpTls {
    input: PreparedLinkSysTls,
    endpoint: OtlpEndpoint,
}

impl PreparedOtlpTls {
    pub fn from_link_sys(owner: Arc<LinkSys>) -> anyhow::Result<Self> {
        Self::with_snapshot(owner, outbound_global_default())
    }

    pub fn with_snapshot(owner: Arc<LinkSys>, defaults: Arc<GlobalOutboundTlsSnapshot>) -> anyhow::Result<Self> {
        let input = PreparedLinkSysTls::with_snapshot(owner, defaults)?;
        let SystemConfig::Otlp(config) = &input.owner().spec.config else {
            anyhow::bail!("OTLP preparation requires an actual OTLP LinkSys");
        };
        // Preserve the provider's explicit namespace/name and config contract;
        // generic source defaults must not relax OTLP's existing validation.
        anyhow::ensure!(input.owner().validate_config().is_empty(), "invalid_config");
        let endpoint = config.validate().map_err(anyhow::Error::msg)?;
        if endpoint.tls {
            input.with_tls_selection(|proof, defaults| {
                let empty = OutboundTlsLocal::default();
                let policy = proof.active_field().map_or(&empty, |field| field.policy());
                for axis in defaults.effective_proofs(policy) {
                    axis?;
                }
                Ok(())
            })?;
        }
        Ok(Self { input, endpoint })
    }

    pub fn owner(&self) -> &LinkSys {
        self.input.owner()
    }

    pub fn endpoint(&self) -> &OtlpEndpoint {
        &self.endpoint
    }

    /// Pure masked identity; no materialization, PEM parsing or TLS audit.
    pub fn hash_inheritance(&self, hasher: &mut FingerprintHasher) {
        let SystemConfig::Otlp(config) = &self.owner().spec.config else {
            unreachable!("prepared OTLP provider is immutable")
        };
        let empty = OutboundTlsLocal::default();
        let policy = self.endpoint.tls.then(|| {
            config
                .tls
                .as_ref()
                .filter(|tls| tls.enabled)
                .map_or(&empty, |tls| &tls.outbound)
        });
        hash_tls_inheritance(hasher, policy, self.input.defaults());
    }

    /// The SDK conversion remains in Gateway; Common owns canonical resolution.
    pub fn resolve_tls(&self) -> anyhow::Result<Option<ResolvedOutboundTls>> {
        if !self.endpoint.tls {
            return Ok(None);
        }
        self.input.with_tls_selection(|proof, defaults| {
            let resolved = resolve_validated_outbound_tls_audited(proof, false, "otlp", defaults)
                .map_err(|_| anyhow::anyhow!("invalid_tls"))?;
            resolved.reject_unsupported_identity_policy("otlp")?;
            Ok(Some(resolved))
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/link_sys_core/outbound_tls/otlp_test.rs"]
mod tests;
