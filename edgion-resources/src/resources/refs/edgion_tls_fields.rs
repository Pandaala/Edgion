//! Independent actual EdgionTls server and client-CA Secret fields.
use super::outbound_tls::{BoundTlsMaterial, TlsMaterialIdentityError};
use super::owned::{FieldContract, FieldSlot, FieldWire, OwnerSnapshot, PreparedSlot, TopologyLoan};
use super::ReferenceToken;
use crate::EdgionTls;
use k8s_openapi::api::core::v1::Secret;

/// Borrow the actual resource; preparing one field never validates its sibling.
/// ```
/// use edgion_resources::resources::refs::edgion_tls_fields::EdgionTlsRefs;
/// fn positive(owner: &edgion_resources::EdgionTls) {
///     let refs = EdgionTlsRefs::from_tls(owner);
///     let _ = refs.server().token().validated();
/// }
/// ```
/// ```compile_fail
/// use edgion_resources::resources::refs::edgion_tls_fields::EdgionTlsRefs;
/// fn invalid(owner: edgion_resources::EdgionTls) {
///     let refs = EdgionTlsRefs::from_tls(&owner);
///     let field = refs.server();
///     drop(owner);
///     let _ = field.token().validated();
/// }
/// ```
/// ```compile_fail
/// use edgion_resources::resources::refs::edgion_tls_fields::EdgionTlsRefs;
/// fn invalid(owner: &mut edgion_resources::EdgionTls) {
///     let refs = EdgionTlsRefs::from_tls(owner);
///     let field = refs.server();
///     owner.spec.secret = None;
///     let _ = field.token().validated();
/// }
/// ```
/// ```compile_fail
/// use edgion_resources::resources::refs::edgion_tls_fields::EdgionTlsRefs;
/// fn invalid(owner: &mut edgion_resources::EdgionTls) {
///     let refs = EdgionTlsRefs::from_tls(owner);
///     let field = refs.server();
///     owner.spec.secret_ref.name.clear();
///     let _ = field.token().validated();
/// }
/// ```
pub struct EdgionTlsRefs<'a> {
    tls: &'a EdgionTls,
    owner: OwnerSnapshot,
}
pub struct EdgionTlsField<'a> {
    _tls: &'a EdgionTls,
    owner: &'a OwnerSnapshot,
    slot: PreparedSlot,
    material: Option<&'a Secret>,
}
impl<'a> EdgionTlsRefs<'a> {
    pub fn from_tls(tls: &'a EdgionTls) -> Self {
        Self {
            tls,
            owner: OwnerSnapshot::namespaced("edgion.io", "EdgionTls", &tls.metadata),
        }
    }
    pub fn server(&self) -> EdgionTlsField<'_> {
        EdgionTlsField {
            _tls: self.tls,
            owner: &self.owner,
            slot: PreparedSlot::new(
                &self.owner,
                FieldWire::Secret(self.tls.spec.secret_ref.clone()),
                FieldContract::StrictSecret,
                FieldSlot::EdgionTlsServer,
            ),
            material: self.tls.spec.secret.as_ref(),
        }
    }
    pub fn client_ca(&self) -> Option<EdgionTlsField<'_>> {
        let auth = self.tls.spec.client_auth.as_ref()?;
        let reference = auth.ca_secret_ref.as_ref()?;
        Some(EdgionTlsField {
            _tls: self.tls,
            owner: &self.owner,
            slot: PreparedSlot::new(
                &self.owner,
                FieldWire::Secret(reference.clone()),
                FieldContract::StrictSecret,
                FieldSlot::EdgionTlsClientCa,
            ),
            material: auth.ca_secret.as_ref(),
        })
    }
}
impl EdgionTlsField<'_> {
    pub fn token(&self) -> ReferenceToken<'_> {
        self.slot.token(TopologyLoan::EdgionTls(self.owner))
    }
    pub fn material(&self) -> Result<BoundTlsMaterial<'_>, TlsMaterialIdentityError> {
        let pair = BoundTlsMaterial::from_edgion_tls(self, self.material);
        pair.validate_identity()?;
        Ok(pair)
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/edgion_tls_fields_test.rs"]
mod tests;
