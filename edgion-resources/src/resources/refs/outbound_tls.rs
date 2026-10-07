//! Actual-owner local TLS policy, reference and material loans.
//!
//! This is an in-process capability, not a transport schema or a global TLS
//! inheritance proof. Constructors belong to the owning field modules.
use super::link_sys_fields::{LinkSysReferences, LinkSysTlsField};
use super::plugin_fields::{EntryReferences, PluginTlsField};
use super::{ReferenceError, ReferenceToken};
use crate::resources::common::{OutboundTlsConfig, OutboundTlsLocal};
use k8s_openapi::api::core::v1::Secret;

enum Origin<'a> {
    Plugin(PluginTlsField<'a>),
    LinkSys(LinkSysTlsField<'a>),
}

enum Selection<'a> {
    Active(BoundOutboundTls<'a>),
    InactivePlugin(EntryReferences<'a>),
    InactiveLinkSys(LinkSysReferences<'a>),
}

/// The actual local TLS selection, including an absent or inactive field.
/// This loan does not enable transport or authorize client construction.
/// ```compile_fail
/// use edgion_resources::resources::refs::outbound_tls::BoundLocalTlsSelection;
/// let selection = BoundLocalTlsSelection {};
/// ```
/// Even an absent field retains the actual owner's borrow.
/// ```compile_fail
/// use edgion_resources::resources::refs::link_sys_fields::PreparedLinkSysReferences;
/// fn overlap(prepared: &PreparedLinkSysReferences, owner: &mut edgion_resources::LinkSys) {
///     let mut binding = prepared.bind_mut(owner).unwrap();
///     let selection = binding.outbound_tls_selection().unwrap();
///     binding.config_mut();
///     selection.validate_source().unwrap();
/// }
/// ```
/// An inactive selection cannot outlive its owner.
/// ```compile_fail
/// use edgion_resources::resources::refs::link_sys_fields::PreparedLinkSysReferences;
/// fn overlap(prepared: &PreparedLinkSysReferences, owner: edgion_resources::LinkSys) {
///     let binding = prepared.bind(&owner).unwrap();
///     let selection = binding.outbound_tls_selection().unwrap();
///     drop(owner);
///     selection.validate_source().unwrap();
/// }
/// ```
pub struct BoundLocalTlsSelection<'a> {
    selection: Selection<'a>,
}
impl std::fmt::Debug for BoundLocalTlsSelection<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("BoundLocalTlsSelection").finish_non_exhaustive()
    }
}
impl<'a> BoundLocalTlsSelection<'a> {
    pub(super) fn from_active(field: BoundOutboundTls<'a>) -> Self {
        Self {
            selection: Selection::Active(field),
        }
    }
    pub(super) fn from_plugin(references: EntryReferences<'a>) -> Self {
        Self {
            selection: Selection::InactivePlugin(references),
        }
    }
    pub(super) fn from_link_sys(references: LinkSysReferences<'a>) -> Self {
        Self {
            selection: Selection::InactiveLinkSys(references),
        }
    }
    pub fn active_field(&self) -> Option<&BoundOutboundTls<'a>> {
        match &self.selection {
            Selection::Active(field) => Some(field),
            Selection::InactivePlugin(_) | Selection::InactiveLinkSys(_) => None,
        }
    }
    pub fn validate_source(&self) -> Result<(), ReferenceError> {
        match &self.selection {
            Selection::Active(field) => field.validate_source(),
            Selection::InactivePlugin(references) => references.validate_source(),
            Selection::InactiveLinkSys(references) => references.validate_source(),
        }
    }
}

/// One active local field, borrowed from its actual resource.
///
/// An external caller cannot fabricate a loan from unrelated policy/material.
/// ```compile_fail
/// use edgion_resources::resources::refs::outbound_tls::BoundOutboundTls;
/// let field = BoundOutboundTls {};
/// ```
pub struct BoundOutboundTls<'a> {
    origin: Origin<'a>,
}

/// A declared reference and the material at the same author-ordered position.
pub struct BoundTlsMaterial<'a> {
    reference: ReferenceToken<'a>,
    material: Option<&'a Secret>,
}
impl BoundTlsMaterial<'_> {
    pub fn reference(&self) -> &ReferenceToken<'_> {
        &self.reference
    }
    pub fn material(&self) -> Option<&Secret> {
        self.material
    }
}

/// The actual cluster EGC policy and its paired material; no local-policy copy.
/// ```compile_fail
/// use edgion_resources::resources::refs::outbound_tls::BoundGlobalOutboundTls;
/// let field = BoundGlobalOutboundTls {};
/// ```
/// A global policy loan prevents material mutation while it is in use.
/// ```compile_fail
/// use edgion_resources::resources::refs::egc_tls_fields::PreparedEgcTlsReferences;
/// fn overlap(prepared: &PreparedEgcTlsReferences, owner: &mut edgion_resources::EdgionGatewayConfig) {
///     let mut binding = prepared.bind_mut(owner).unwrap();
///     let field = binding.outbound_tls().unwrap().unwrap();
///     binding.outbound_tls_mut();
///     field.policy();
/// }
/// ```
pub struct BoundGlobalOutboundTls<'a> {
    field: super::egc_tls_fields::EgcTlsField<'a>,
}
impl BoundGlobalOutboundTls<'_> {
    pub(super) fn from_egc(field: super::egc_tls_fields::EgcTlsField<'_>) -> BoundGlobalOutboundTls<'_> {
        BoundGlobalOutboundTls { field }
    }
    pub fn policy(&self) -> &OutboundTlsConfig {
        self.field.policy
    }
    pub fn ca_material(&self) -> Option<&[Secret]> {
        self.policy().resolved_ca_certificates.as_deref()
    }
    pub fn client_material(&self) -> Option<&Secret> {
        self.policy().resolved_client_certificate.as_ref()
    }
    pub fn ca_reference_count(&self) -> usize {
        self.policy()
            .validation
            .as_ref()
            .and_then(|validation| validation.ca_certificate_refs.as_ref())
            .map_or(0, Vec::len)
    }
    pub fn visit_ca<'loan>(&'loan self, mut visit: impl FnMut(BoundTlsMaterial<'loan>)) {
        let material = self.ca_material().unwrap_or_default();
        let mut ordinal = 0;
        self.field.visit_ca(|reference| {
            visit(BoundTlsMaterial {
                reference,
                material: material.get(ordinal),
            });
            ordinal += 1;
        });
    }
    pub fn client_certificate(&self) -> Option<BoundTlsMaterial<'_>> {
        self.field.client_reference().map(|reference| BoundTlsMaterial {
            reference,
            material: self.client_material(),
        })
    }
}

impl<'a> BoundOutboundTls<'a> {
    pub(super) fn from_plugin(field: PluginTlsField<'a>) -> Self {
        Self {
            origin: Origin::Plugin(field),
        }
    }
    pub(super) fn from_link_sys(field: LinkSysTlsField<'a>) -> Self {
        Self {
            origin: Origin::LinkSys(field),
        }
    }
    pub fn validate_source(&self) -> Result<(), ReferenceError> {
        match &self.origin {
            Origin::Plugin(field) => field.validate_source(),
            Origin::LinkSys(field) => field.validate_source(),
        }
    }
    pub fn policy(&self) -> &OutboundTlsLocal {
        match &self.origin {
            Origin::Plugin(field) => field.policy(),
            Origin::LinkSys(field) => field.policy(),
        }
    }
    pub fn ca_material(&self) -> Option<&[Secret]> {
        match &self.origin {
            Origin::Plugin(field) => field.ca_material(),
            Origin::LinkSys(field) => field.ca_material(),
        }
    }
    pub fn client_material(&self) -> Option<&Secret> {
        match &self.origin {
            Origin::Plugin(field) => field.client_material(),
            Origin::LinkSys(field) => field.client_material(),
        }
    }
    pub fn ca_reference_count(&self) -> usize {
        self.policy()
            .validation
            .as_ref()
            .and_then(|validation| validation.ca_certificate_refs.as_ref())
            .map_or(0, Vec::len)
    }
    /// Visits every declared slot, including invalid or unsupported targets.
    /// No caller-provided collection can filter, reorder or replace the pairs.
    pub fn visit_ca<'loan>(&'loan self, mut visit: impl FnMut(BoundTlsMaterial<'loan>)) {
        let material = self.ca_material().unwrap_or_default();
        let mut ordinal = 0;
        let mut pair = |reference| {
            visit(BoundTlsMaterial {
                reference,
                material: material.get(ordinal),
            });
            ordinal += 1;
        };
        match &self.origin {
            Origin::Plugin(field) => field.visit_ca(&mut pair),
            Origin::LinkSys(field) => field.visit_ca(&mut pair),
        }
    }
    pub fn client_certificate(&self) -> Option<BoundTlsMaterial<'_>> {
        let reference = match &self.origin {
            Origin::Plugin(field) => field.client_reference(),
            Origin::LinkSys(field) => field.client_reference(),
        }?;
        Some(BoundTlsMaterial {
            reference,
            material: self.client_material(),
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/outbound_tls_test.rs"]
mod tests;
