//! Actual-owner local TLS policy, reference and material loans.
//!
//! This is an in-process capability, not a transport schema or a global TLS
//! inheritance proof. Constructors belong to the owning field modules.
use super::link_sys_fields::{LinkSysReferences, LinkSysTlsField};
use super::owned::PreparedTlsTarget;
use super::plugin_fields::{EntryReferences, PluginTlsField};
use super::route_auth_fields::{EffectiveRouteAuthField, RouteAuthTlsField};
use super::{ObjectIdentity, ReferenceError, ReferenceToken};
use crate::resources::common::{OutboundTlsConfig, OutboundTlsLocal};
use k8s_openapi::api::core::v1::Secret;

enum Origin<'a> {
    Plugin(PluginTlsField<'a>),
    LinkSys(LinkSysTlsField<'a>),
    RouteAuth(RouteAuthTlsField<'a>),
    Health(super::health_tls_fields::HealthTlsField<'a>),
}

enum Selection<'a> {
    Active(BoundOutboundTls<'a>),
    InactivePlugin(EntryReferences<'a>),
    InactiveLinkSys(LinkSysReferences<'a>),
    InactiveRouteAuth(&'a EffectiveRouteAuthField<'a>),
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
    pub(super) fn from_route_auth(field: &'a EffectiveRouteAuthField<'a>) -> Self {
        Self {
            selection: Selection::InactiveRouteAuth(field),
        }
    }
    pub fn active_field(&self) -> Option<&BoundOutboundTls<'a>> {
        match &self.selection {
            Selection::Active(field) => Some(field),
            Selection::InactivePlugin(_) | Selection::InactiveLinkSys(_) | Selection::InactiveRouteAuth(_) => None,
        }
    }
    pub fn validate_source(&self) -> Result<(), ReferenceError> {
        match &self.selection {
            Selection::Active(field) => field.validate_source(),
            Selection::InactivePlugin(references) => references.validate_source(),
            Selection::InactiveLinkSys(references) => references.validate_source(),
            Selection::InactiveRouteAuth(field) => field.validate_source(),
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

/// A declared reference and the material selected for its actual field.
pub struct BoundTlsMaterial<'a> {
    reference: TlsReferenceIdentity<'a>,
    material: Option<&'a Secret>,
}
impl BoundTlsMaterial<'_> {
    pub fn reference(&self) -> &TlsReferenceIdentity<'_> {
        &self.reference
    }
    pub fn material(&self) -> Option<&Secret> {
        self.material
    }
}

/// A TLS target projection. It grants no acquisition or ReferenceGrant authority.
/// ```compile_fail
/// use edgion_resources::resources::refs::outbound_tls::TlsReferenceIdentity;
/// fn no_grant(field: &TlsReferenceIdentity<'_>) {
///     field.validated().unwrap().grant_query();
/// }
/// ```
pub struct TlsReferenceIdentity<'a> {
    origin: TlsIdentityOrigin<'a>,
}
enum TlsIdentityOrigin<'a> {
    Authored(ReferenceToken<'a>),
    Effective(&'a PreparedTlsTarget),
}
pub struct ValidatedTlsIdentity<'a> {
    target: ObjectIdentity<'a>,
}
impl<'a> ValidatedTlsIdentity<'a> {
    pub fn target(&self) -> ObjectIdentity<'a> {
        self.target
    }
}
impl TlsReferenceIdentity<'_> {
    pub fn validated(&self) -> Result<ValidatedTlsIdentity<'_>, ReferenceError> {
        let target = match &self.origin {
            TlsIdentityOrigin::Authored(token) => token.validated()?.target(),
            TlsIdentityOrigin::Effective(slot) => slot.target()?,
        };
        Ok(ValidatedTlsIdentity { target })
    }
}
impl<'a> TlsReferenceIdentity<'a> {
    fn authored(token: ReferenceToken<'a>) -> Self {
        Self {
            origin: TlsIdentityOrigin::Authored(token),
        }
    }
}
impl BoundTlsMaterial<'_> {
    pub fn target(&self) -> Result<ObjectIdentity<'_>, ReferenceError> {
        Ok(self.reference.validated()?.target())
    }
}

/// Material-free identity failures shared by all actual TLS field consumers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsMaterialIdentityError {
    Reference(ReferenceError),
    Identity,
    Provenance,
}
impl<'a> BoundTlsMaterial<'a> {
    pub(super) fn from_header_ca(token: ReferenceToken<'a>, material: &'a Secret) -> Self {
        Self {
            reference: TlsReferenceIdentity::authored(token),
            material: Some(material),
        }
    }

    pub(super) fn from_gateway_backend(
        field: &'a super::gateway_tls_fields::GatewayTlsField<'_>,
        material: Option<&'a Secret>,
    ) -> Self {
        Self {
            reference: TlsReferenceIdentity::authored(field.token()),
            material,
        }
    }
    pub(super) fn from_edgion_tls(
        field: &'a super::edgion_tls_fields::EdgionTlsField<'_>,
        material: Option<&'a Secret>,
    ) -> Self {
        Self {
            reference: TlsReferenceIdentity::authored(field.token()),
            material,
        }
    }
    pub(super) fn from_backend_policy(
        field: &'a super::backend_tls_fields::BackendTlsField<'_>,
        material: Option<&'a Secret>,
    ) -> Self {
        Self {
            reference: TlsReferenceIdentity::authored(field.token()),
            material,
        }
    }
    /// Check the actual declared target and material provenance without reading stores.
    pub fn validate_identity(&self) -> Result<(), TlsMaterialIdentityError> {
        use crate::constants::annotations::edgion::{CONTROLLER_CA_SOURCE, CONTROLLER_CA_SOURCE_CONFIG_MAP};
        let target = self.target().map_err(TlsMaterialIdentityError::Reference)?;
        let material = self.material.ok_or(TlsMaterialIdentityError::Identity)?;
        let local_default = match &self.reference.origin {
            TlsIdentityOrigin::Authored(token) => {
                token
                    .validated()
                    .map_err(TlsMaterialIdentityError::Reference)?
                    .is_backend_tls_material()
                    || token
                        .validated()
                        .map_err(TlsMaterialIdentityError::Reference)?
                        .is_gateway_backend_tls_material()
                    || token
                        .validated()
                        .map_err(TlsMaterialIdentityError::Reference)?
                        .is_health_tls_material()
                    || token
                        .validated()
                        .map_err(TlsMaterialIdentityError::Reference)?
                        .is_edgion_tls_material()
            }
            TlsIdentityOrigin::Effective(_) => false,
        };
        let namespace = if local_default {
            Some(material.metadata.namespace.as_deref().unwrap_or("default"))
        } else {
            material.metadata.namespace.as_deref()
        };
        if material.metadata.name.as_deref() != Some(target.name()) || namespace != target.scope().namespace() {
            return Err(TlsMaterialIdentityError::Identity);
        }
        let marker = material
            .metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get(CONTROLLER_CA_SOURCE));
        let matches = match target.group_kind().kind() {
            "Secret" => marker.is_none(),
            "ConfigMap" => marker.is_some_and(|v| v == CONTROLLER_CA_SOURCE_CONFIG_MAP),
            _ => false,
        };
        if !matches {
            return Err(TlsMaterialIdentityError::Provenance);
        }
        Ok(())
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
                reference: TlsReferenceIdentity::authored(reference),
                material: material.get(ordinal),
            });
            ordinal += 1;
        });
    }
    pub fn client_certificate(&self) -> Option<BoundTlsMaterial<'_>> {
        self.field.client_reference().map(|reference| BoundTlsMaterial {
            reference: TlsReferenceIdentity::authored(reference),
            material: self.client_material(),
        })
    }
}

impl<'a> BoundOutboundTls<'a> {
    pub(super) fn from_health(field: super::health_tls_fields::HealthTlsField<'a>) -> Self {
        Self {
            origin: Origin::Health(field),
        }
    }
    pub(super) fn from_route_auth(field: RouteAuthTlsField<'a>) -> Self {
        Self {
            origin: Origin::RouteAuth(field),
        }
    }
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
            Origin::RouteAuth(field) => field.validate_source(),
            Origin::Health(field) => field.validate_source(),
        }
    }
    pub fn policy(&self) -> &OutboundTlsLocal {
        match &self.origin {
            Origin::Plugin(field) => field.policy(),
            Origin::LinkSys(field) => field.policy(),
            Origin::RouteAuth(field) => field.policy(),
            Origin::Health(field) => field.policy(),
        }
    }
    pub fn ca_material(&self) -> Option<&[Secret]> {
        match &self.origin {
            Origin::Plugin(field) => field.ca_material(),
            Origin::LinkSys(field) => field.ca_material(),
            Origin::RouteAuth(field) => field.ca_material(),
            Origin::Health(field) => field.ca_material(),
        }
    }
    pub fn client_material(&self) -> Option<&Secret> {
        match &self.origin {
            Origin::Plugin(field) => field.client_material(),
            Origin::LinkSys(field) => field.client_material(),
            Origin::RouteAuth(field) => field.client_material(),
            Origin::Health(field) => field.client_material(),
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
        let mut pair = |reference: TlsReferenceIdentity<'loan>| {
            visit(BoundTlsMaterial {
                reference,
                material: material.get(ordinal),
            });
            ordinal += 1;
        };
        match &self.origin {
            Origin::Health(field) => field.visit_ca(|reference| pair(TlsReferenceIdentity::authored(reference))),
            Origin::Plugin(field) => field.visit_ca(|reference| pair(TlsReferenceIdentity::authored(reference))),
            Origin::LinkSys(field) => field.visit_ca(|reference| pair(TlsReferenceIdentity::authored(reference))),
            Origin::RouteAuth(field) => field.visit_ca(|slot| {
                pair(TlsReferenceIdentity {
                    origin: TlsIdentityOrigin::Effective(slot),
                })
            }),
        }
    }
    pub fn client_certificate(&self) -> Option<BoundTlsMaterial<'_>> {
        let reference = match &self.origin {
            Origin::Health(field) => field.client_reference().map(TlsReferenceIdentity::authored),
            Origin::Plugin(field) => field.client_reference().map(TlsReferenceIdentity::authored),
            Origin::LinkSys(field) => field.client_reference().map(TlsReferenceIdentity::authored),
            Origin::RouteAuth(field) => field.client_reference().map(|slot| TlsReferenceIdentity {
                origin: TlsIdentityOrigin::Effective(slot),
            }),
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
