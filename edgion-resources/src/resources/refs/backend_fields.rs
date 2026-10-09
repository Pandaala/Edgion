//! Actual EdgionBackend fields; existing schema validation owns target grammar.
use super::{ObjectIdentity, ReferenceError, ResourceScope};
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

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/backend_fields_test.rs"]
mod tests;
