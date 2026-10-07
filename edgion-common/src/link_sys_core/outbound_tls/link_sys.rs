//! Shared immutable actual LinkSys reference ownership and captured defaults.
use std::sync::Arc;

use edgion_resources::resources::link_sys::LinkSys;
use edgion_resources::resources::refs::link_sys_fields::PreparedLinkSysReferences;

use super::{
    outbound_global_default, validate_active_tls_inheritance, validate_local_tls_selection, GlobalOutboundTlsSnapshot,
    ValidatedLocalTlsSelection,
};
use crate::link_sys_core::fingerprint::{hash_tls_inheritance, FingerprintHasher};

/// Actual source and whole active local pairing, without transport enablement.
/// Closed owning provider adapters decide which inherited axes they consume.
/// ```compile_fail
/// use edgion_common::link_sys_core::outbound_tls::PreparedLinkSysTls;
/// let prepared = PreparedLinkSysTls {};
/// ```
/// A callback cannot release its owning input while using the borrowed proof.
/// ```compile_fail
/// use std::sync::Arc;
/// use edgion_common::link_sys_core::outbound_tls::PreparedLinkSysTls;
/// let owner = serde_json::from_value(serde_json::json!({"metadata":{"name":"es"},
///     "spec":{"type":"elasticsearch","config":{"endpoints":["https://es.example:9200"]}}})).unwrap();
/// let prepared = PreparedLinkSysTls::from_link_sys(Arc::new(owner)).unwrap();
/// prepared.with_tls_selection(|proof, _| { drop(prepared); let _ = proof.active_field(); Ok(()) });
/// ```
pub struct PreparedLinkSysTls {
    owner: Arc<LinkSys>,
    references: PreparedLinkSysReferences,
    defaults: Arc<GlobalOutboundTlsSnapshot>,
}

impl PreparedLinkSysTls {
    pub fn from_link_sys(owner: Arc<LinkSys>) -> anyhow::Result<Self> {
        Self::with_snapshot(owner, outbound_global_default())
    }

    pub fn with_snapshot(owner: Arc<LinkSys>, defaults: Arc<GlobalOutboundTlsSnapshot>) -> anyhow::Result<Self> {
        let references = PreparedLinkSysReferences::from_link_sys(&owner);
        let prepared = Self {
            owner,
            references,
            defaults,
        };
        prepared.with_tls_selection(|_, _| Ok(()))?;
        Ok(prepared)
    }

    pub fn owner(&self) -> &LinkSys {
        &self.owner
    }

    pub(crate) fn defaults(&self) -> &GlobalOutboundTlsSnapshot {
        &self.defaults
    }

    /// The callback's proof borrows the actual retained owner and reference
    /// topology. No detached local policy or material can replace its inputs.
    pub fn with_tls_selection<R>(
        &self,
        apply: impl FnOnce(&ValidatedLocalTlsSelection<'_>, &GlobalOutboundTlsSnapshot) -> anyhow::Result<R>,
    ) -> anyhow::Result<R> {
        let binding = self.references.bind(&self.owner)?;
        let proof = validate_local_tls_selection(binding.outbound_tls_selection()?)?;
        apply(&proof, &self.defaults)
    }

    /// Native inactive HTTP policy consumes no inherited axes.
    pub fn validate_active_inheritance(&self) -> anyhow::Result<()> {
        self.with_tls_selection(|proof, defaults| {
            validate_active_tls_inheritance(proof, defaults)?;
            Ok(())
        })
    }

    pub fn hash_active_inheritance(&self, hasher: &mut FingerprintHasher) -> anyhow::Result<()> {
        self.with_tls_selection(|proof, defaults| {
            hash_tls_inheritance(hasher, proof.active_field().map(|field| field.policy()), defaults);
            Ok(())
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/link_sys_core/outbound_tls/link_sys_test.rs"]
mod tests;
