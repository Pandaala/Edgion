//! Actual BackendTLSPolicy CA ordinals and same-namespace client option material.
use super::outbound_tls::{BoundTlsMaterial, TlsMaterialIdentityError};
use super::owned::{FieldContract, FieldSlot, FieldWire, OwnerSnapshot, PreparedSlot, TopologyLoan};
use super::{ReferenceError, ReferenceToken};
use crate::constants::secret_keys::tls::{CA_CERT, CERT, KEY};
use crate::resources::backend_tls_policy::{parse_client_certificate_option, WellKnownCACertificates};
use crate::BackendTLSPolicy;
use k8s_openapi::api::core::v1::Secret;

/// Native Gateway API caCertificateRefs maxItems, shared by all BTP consumers.
pub const MAX_BACKEND_TLS_CA_REFERENCES: usize = 8;

/// Immutable actual policy owner; no copied policy or resolved payload.
/// An active field cannot outlive its real owner.
/// ```compile_fail
/// use edgion_resources::resources::refs::backend_tls_fields::BackendTlsRefs;
/// fn invalid(owner: edgion_resources::BackendTLSPolicy) {
///     let refs = BackendTlsRefs::from_policy(&owner);
///     let field = refs.ca(0).unwrap();
///     drop(owner);
///     field.token().validated().unwrap();
/// }
/// ```
/// Material mutation and declaration reordering cannot overlap a live loan.
/// ```compile_fail
/// use edgion_resources::resources::refs::backend_tls_fields::BackendTlsRefs;
/// fn invalid(owner: &mut edgion_resources::BackendTLSPolicy) {
///     let refs = BackendTlsRefs::from_policy(owner);
///     let field = refs.ca(0).unwrap();
///     owner.spec.resolved_ca_certificates = None;
///     field.material().unwrap();
/// }
/// ```
/// ```compile_fail
/// use edgion_resources::resources::refs::backend_tls_fields::BackendTlsRefs;
/// fn invalid(owner: &mut edgion_resources::BackendTLSPolicy) {
///     let refs = BackendTlsRefs::from_policy(owner);
///     let field = refs.ca(0).unwrap();
///     owner.spec.validation.ca_certificate_refs.as_mut().unwrap().swap(0, 1);
///     field.token().validated().unwrap();
/// }
/// ```
pub struct BackendTlsRefs<'a> {
    policy: &'a BackendTLSPolicy,
    owner: OwnerSnapshot,
}

/// One declared CA ordinal or the actual client-certificate option.
pub struct BackendTlsField<'a> {
    policy: &'a BackendTLSPolicy,
    owner: &'a OwnerSnapshot,
    slot: PreparedSlot,
    client: bool,
}

impl<'a> BackendTlsRefs<'a> {
    pub fn from_policy(policy: &'a BackendTLSPolicy) -> Self {
        Self {
            policy,
            owner: OwnerSnapshot::namespaced("gateway.networking.k8s.io", "BackendTLSPolicy", &policy.metadata),
        }
    }
    pub fn ca_count(&self) -> usize {
        self.policy
            .spec
            .validation
            .ca_certificate_refs
            .as_ref()
            .map_or(0, Vec::len)
    }
    /// Constant-time bounds precede every material search or field preparation.
    pub fn bounded_materials(&self) -> bool {
        self.ca_count() <= MAX_BACKEND_TLS_CA_REFERENCES
            && self.policy.spec.resolved_ca_certificates.as_ref().map_or(0, Vec::len) <= self.ca_count()
    }
    pub fn ca(&self, index: usize) -> Option<BackendTlsField<'_>> {
        if !self.bounded_materials() {
            return None;
        }
        let reference = self.policy.spec.validation.ca_certificate_refs.as_ref()?.get(index)?;
        Some(BackendTlsField {
            policy: self.policy,
            owner: &self.owner,
            client: false,
            slot: PreparedSlot::new(
                &self.owner,
                FieldWire::BackendCa(reference.clone()),
                FieldContract::NativeCa,
                FieldSlot::BackendTlsCa { reference: index },
            ),
        })
    }
    pub fn client(&self) -> Result<Option<BackendTlsField<'_>>, ReferenceError> {
        if !self.bounded_materials() {
            return Err(ReferenceError::UnsupportedTarget);
        }
        let Some(reference) = parse_client_certificate_option(self.policy.spec.options.as_ref())
            .map_err(|_| ReferenceError::InvalidName)?
        else {
            return Ok(None);
        };
        Ok(Some(BackendTlsField {
            policy: self.policy,
            owner: &self.owner,
            client: true,
            slot: PreparedSlot::new(
                &self.owner,
                FieldWire::Secret(reference),
                FieldContract::CustomSecret,
                FieldSlot::BackendTlsClient,
            ),
        }))
    }
    /// Complete cold publication proof. Status can separately inspect bounded partial pairs.
    pub fn all_materials_resolved(&self) -> bool {
        if !self.bounded_materials() || self.owner.source().is_err() {
            return false;
        }
        let count = self.ca_count();
        if self.policy.spec.resolved_ca_certificates.as_ref().map_or(0, Vec::len) != count {
            return false;
        }
        let system = matches!(
            self.policy.spec.validation.well_known_ca_certificates,
            Some(WellKnownCACertificates::System)
        );
        if self.policy.spec.use_system_ca != system
            || (system && count != 0)
            || (!system && (count == 0 || self.policy.spec.validation.well_known_ca_certificates.is_some()))
        {
            return false;
        }
        let declarations = self
            .policy
            .spec
            .validation
            .ca_certificate_refs
            .as_deref()
            .unwrap_or_default();
        for index in 0..count {
            // Match the existing Controller duplicate-declaration rejection without allocation.
            if declarations[..index].contains(&declarations[index]) {
                return false;
            }
            if !self.ca(index).is_some_and(|field| field.resolved()) {
                return false;
            }
        }
        match self.client() {
            Ok(Some(field)) => field.resolved(),
            Ok(None) => self.policy.spec.resolved_client_certificate.is_none(),
            Err(_) => false,
        }
    }
}
impl BackendTlsField<'_> {
    pub fn token(&self) -> ReferenceToken<'_> {
        self.slot.token(TopologyLoan::BackendTls(self.owner))
    }
    pub fn material(&self) -> Result<BoundTlsMaterial<'_>, TlsMaterialIdentityError> {
        if self.client {
            let pair =
                BoundTlsMaterial::from_backend_policy(self, self.policy.spec.resolved_client_certificate.as_ref());
            pair.validate_identity()?;
            return Ok(pair);
        }
        // The facade admitted at most eight declarations and no surplus materials.
        self.token().validated().map_err(TlsMaterialIdentityError::Reference)?;
        let mut selected = None;
        for material in self.policy.spec.resolved_ca_certificates.as_deref().unwrap_or_default() {
            if BoundTlsMaterial::from_backend_policy(self, Some(material))
                .validate_identity()
                .is_ok()
            {
                if selected.is_some() {
                    return Err(TlsMaterialIdentityError::Identity);
                }
                selected = Some(material);
            }
        }
        let pair = BoundTlsMaterial::from_backend_policy(self, selected);
        pair.validate_identity()?;
        Ok(pair)
    }
    pub fn resolved(&self) -> bool {
        self.material()
            .ok()
            .and_then(|pair| {
                pair.material().map(|material| {
                    if self.client {
                        has_payload(material, CERT) && has_payload(material, KEY)
                    } else {
                        has_payload(material, CA_CERT)
                    }
                })
            })
            .unwrap_or(false)
    }
}
fn has_payload(material: &Secret, key: &str) -> bool {
    material
        .data
        .as_ref()
        .and_then(|data| data.get(key))
        .is_some_and(|value| !value.0.is_empty())
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/backend_tls_fields_test.rs"]
mod tests;
