//! Health-local TLS fields borrowed from the actual declaring policy.
use super::outbound_tls::BoundOutboundTls;
use super::owned::{FieldContract, FieldSlot, FieldWire, OwnerSnapshot, PreparedSlot, TopologyLoan};
use super::{ReferenceError, ReferenceToken};
use crate::resources::common::OutboundTlsLocal;
use crate::resources::health_check::{ActiveHealthCheckConfig, HealthCheckType};
use crate::EdgionBackendTrafficPolicy;
use k8s_openapi::api::core::v1::Secret;

/// Actual policy ownership remains borrowed throughout field/material validation.
/// ```
/// use edgion_resources::resources::refs::health_tls_fields::HealthTlsRefs;
/// fn positive(owner: &edgion_resources::EdgionBackendTrafficPolicy) {
///     let refs = HealthTlsRefs::from_policy(owner);
///     if let Ok(Some(field)) = refs.field() { field.validate_source().unwrap(); }
/// }
/// ```
/// ```compile_fail
/// use edgion_resources::resources::refs::health_tls_fields::HealthTlsRefs;
/// fn invalid(owner: edgion_resources::EdgionBackendTrafficPolicy) {
///     let refs = HealthTlsRefs::from_policy(&owner);
///     let field = refs.field().unwrap().unwrap();
///     drop(owner);
///     field.validate_source().unwrap();
/// }
/// ```
/// ```compile_fail
/// use edgion_resources::resources::refs::health_tls_fields::HealthTlsRefs;
/// fn invalid(owner: &mut edgion_resources::EdgionBackendTrafficPolicy) {
///     let refs = HealthTlsRefs::from_policy(owner);
///     let field = refs.field().unwrap().unwrap();
///     owner.spec.health_check = None;
///     field.validate_source().unwrap();
/// }
/// ```
/// ```compile_fail
/// use edgion_resources::resources::refs::health_tls_fields::HealthTlsRefs;
/// fn invalid(owner: &mut edgion_resources::EdgionBackendTrafficPolicy) {
///     let refs = HealthTlsRefs::from_policy(owner);
///     let field = refs.field().unwrap().unwrap();
///     owner.spec.health_check.as_mut().unwrap().active.as_mut().unwrap()
///         .tls.as_mut().unwrap().validation.as_mut().unwrap()
///         .ca_certificate_refs.as_mut().unwrap().swap(0, 1);
///     field.validate_source().unwrap();
/// }
/// ```
pub struct HealthTlsRefs<'a> {
    policy: &'a EdgionBackendTrafficPolicy,
    owner: OwnerSnapshot,
    ca: Vec<PreparedSlot>,
    client: Option<PreparedSlot>,
}
pub struct HealthTlsField<'a> {
    refs: &'a HealthTlsRefs<'a>,
    active: &'a ActiveHealthCheckConfig,
    local: &'a OutboundTlsLocal,
}
impl<'a> HealthTlsRefs<'a> {
    pub fn from_policy(policy: &'a EdgionBackendTrafficPolicy) -> Self {
        let owner = OwnerSnapshot::namespaced("edgion.io", "EdgionBackendTrafficPolicy", &policy.metadata);
        let active = policy.spec.health_check.as_ref().and_then(|h| h.active.as_ref());
        let local = active
            .filter(|a| matches!(a.r#type, HealthCheckType::Https | HealthCheckType::Grpcs))
            .and_then(|a| a.tls.as_ref());
        let ca = local
            .and_then(|l| l.validation.as_ref())
            .and_then(|v| v.ca_certificate_refs.as_ref())
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(index, wire)| {
                PreparedSlot::new(
                    &owner,
                    FieldWire::Ca(wire.clone()),
                    FieldContract::CustomCa,
                    FieldSlot::HealthTls { ca: Some(index) },
                )
            })
            .collect();
        let client = local.and_then(|l| l.client_certificate_ref.as_ref()).map(|wire| {
            PreparedSlot::new(
                &owner,
                FieldWire::Secret(wire.clone()),
                FieldContract::CustomSecret,
                FieldSlot::HealthTls { ca: None },
            )
        });
        Self {
            policy,
            owner,
            ca,
            client,
        }
    }
    pub fn field(&self) -> Result<Option<HealthTlsField<'_>>, ReferenceError> {
        let Some(active) = self.policy.spec.health_check.as_ref().and_then(|h| h.active.as_ref()) else {
            return Ok(None);
        };
        if !matches!(active.r#type, HealthCheckType::Https | HealthCheckType::Grpcs) {
            return Ok(None);
        }
        self.owner.source()?;
        if active.tls_validation_error().is_some()
            || active
                .tls_namespace_validation_error(self.policy.metadata.namespace.as_deref().unwrap_or("default"))
                .is_some()
        {
            return Err(ReferenceError::UnsupportedTarget);
        }
        let local = active.tls.as_ref().ok_or(ReferenceError::UnsupportedTarget)?;
        Ok(Some(HealthTlsField {
            refs: self,
            active,
            local,
        }))
    }
}
impl<'a> HealthTlsField<'a> {
    pub fn outbound(self) -> BoundOutboundTls<'a> {
        BoundOutboundTls::from_health(self)
    }
    pub fn validate_source(&self) -> Result<(), ReferenceError> {
        self.refs.owner.source().map(|_| ())
    }
    pub fn policy(&self) -> &OutboundTlsLocal {
        self.local
    }
    pub fn ca_material(&self) -> Option<&[Secret]> {
        self.active.resolved_ca_certificates.as_deref()
    }
    pub fn client_material(&self) -> Option<&Secret> {
        self.active.resolved_client_certificate.as_ref()
    }
    pub fn visit_ca<'loan>(&'loan self, mut visit: impl FnMut(ReferenceToken<'loan>)) {
        for slot in &self.refs.ca {
            visit(slot.token(TopologyLoan::HealthTls(&self.refs.owner)));
        }
    }
    pub fn client_reference(&self) -> Option<ReferenceToken<'_>> {
        self.refs
            .client
            .as_ref()
            .map(|slot| slot.token(TopologyLoan::HealthTls(&self.refs.owner)))
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/health_tls_fields_test.rs"]
mod tests;
