//! Actual EdgionBackend fields; existing schema validation owns target grammar.
use super::owned::FieldSlot;
use super::{BoundFieldReference, ObjectIdentity, ReferenceError, ResourceScope};
use crate::EdgionBackend;

/// A borrowed actual credential-pool owner, without a provider cache or Grant.
///
/// ```compile_fail
/// use edgion_resources::EdgionBackend;
/// use edgion_resources::resources::refs::backend_fields::BackendCredentialPoolField;
/// fn cannot_drop_owner(owner: EdgionBackend) {
///     let reference = BackendCredentialPoolField::from_backend(&owner).redis_reference().unwrap();
///     drop(owner);
///     let _ = reference.is_some();
/// }
/// ```
/// Mutation cannot overlap the borrowed field either.
///
/// ```compile_fail
/// use edgion_resources::EdgionBackend;
/// use edgion_resources::resources::refs::backend_fields::BackendCredentialPoolField;
/// fn cannot_mutate_owner(mut owner: EdgionBackend) {
///     let reference = BackendCredentialPoolField::from_backend(&owner).redis_reference().unwrap();
///     owner.metadata.name = None;
///     let _ = reference.is_some();
/// }
/// ```
pub struct BackendCredentialPoolField<'a> {
    owner: &'a EdgionBackend,
}

impl<'a> BackendCredentialPoolField<'a> {
    pub fn from_backend(owner: &'a EdgionBackend) -> Self {
        Self { owner }
    }

    /// Borrow one actual credential Secret field, without retaining a snapshot.
    pub fn secret_reference(self, index: usize) -> BackendSecretField<'a> {
        BackendSecretField {
            owner: self.owner,
            position: FieldSlot::BackendCredential { index },
        }
    }

    /// Validate an authored Redis field's source, preserving its exact target string.
    /// The schema owns limits/ref pairing and lexical validation before compilation.
    pub fn redis_reference(self) -> Result<Option<&'a str>, ReferenceError> {
        let Some(reference) = self.owner.spec.ai.credential_pool.redis_ref.as_deref() else {
            return Ok(None);
        };
        ObjectIdentity::checked(
            "edgion.io",
            "EdgionBackend",
            ResourceScope::Namespaced(self.owner.metadata.namespace.as_deref().unwrap_or("default")),
            self.owner.metadata.name.as_deref().unwrap_or_default(),
        )?;
        Ok(Some(reference))
    }
}

/// An ordinal loan whose target is read only from its actual Backend owner.
///
/// ```compile_fail
/// use edgion_resources::EdgionBackend;
/// use edgion_resources::resources::refs::backend_fields::BackendCredentialPoolField;
/// fn cannot_drop_owner(owner: EdgionBackend) {
///     let field = BackendCredentialPoolField::from_backend(&owner).secret_reference(0);
///     drop(owner);
///     let _ = field.validated();
/// }
/// ```
/// ```compile_fail
/// use edgion_resources::EdgionBackend;
/// use edgion_resources::resources::refs::backend_fields::BackendCredentialPoolField;
/// fn cannot_replace_slot(mut owner: EdgionBackend) {
///     let field = BackendCredentialPoolField::from_backend(&owner).secret_reference(0);
///     owner.spec.ai.credential_pool.credentials.clear();
///     let _ = field.validated();
/// }
/// ```
pub struct BackendSecretField<'a> {
    owner: &'a EdgionBackend,
    position: FieldSlot,
}

impl BackendSecretField<'_> {
    pub fn validated(&self) -> Result<BoundFieldReference<'_>, ReferenceError> {
        BoundFieldReference::backend_secret(self.owner, &self.position)
    }

    /// Borrow only this actual slot's material after checking its reference identity.
    /// A missing raw namespace is not a qualified default-namespace Secret.
    ///
    /// ```compile_fail
    /// use edgion_resources::EdgionBackend;
    /// use edgion_resources::resources::refs::backend_fields::BackendCredentialPoolField;
    /// fn cannot_drop_material_owner(owner: EdgionBackend) {
    ///     let field = BackendCredentialPoolField::from_backend(&owner).secret_reference(0);
    ///     let secret = field.resolved_secret().unwrap();
    ///     drop(owner);
    ///     let _ = secret.is_some();
    /// }
    /// ```
    /// ```compile_fail
    /// use edgion_resources::EdgionBackend;
    /// use edgion_resources::resources::refs::backend_fields::BackendCredentialPoolField;
    /// fn cannot_replace_material(mut owner: EdgionBackend) {
    ///     let field = BackendCredentialPoolField::from_backend(&owner).secret_reference(0);
    ///     let secret = field.resolved_secret().unwrap();
    ///     owner.spec.ai.credential_pool.credentials.clear();
    ///     let _ = secret.is_some();
    /// }
    /// ```
    pub fn resolved_secret(&self) -> Result<Option<&k8s_openapi::api::core::v1::Secret>, ReferenceError> {
        let reference = self.validated()?;
        let FieldSlot::BackendCredential { index } = self.position else {
            return Err(ReferenceError::UnsupportedTarget);
        };
        let slot = self
            .owner
            .spec
            .ai
            .credential_pool
            .credentials
            .get(index)
            .ok_or(ReferenceError::StaleBinding)?;
        let Some(secret) = slot.credential.secret.as_ref() else {
            return Ok(None);
        };
        super::owned::validate_secret_metadata(reference, secret)?;
        Ok(Some(secret))
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/backend_fields_test.rs"]
mod tests;
