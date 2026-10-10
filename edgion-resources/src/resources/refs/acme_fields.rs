//! Actual ACME credential fields; account persistence cannot accept an EAB loan.
use super::owned::{FieldContract, FieldSlot, FieldWire, OwnerSnapshot, PreparedSlot, TopologyLoan};
use super::{ObjectIdentity, ReferenceError, ReferenceToken, ResourceScope};
use crate::EdgionAcme;
use k8s_openapi::api::core::v1::Secret;

/// Actual credential fields remain borrowed from their declaring owner.
/// ```
/// use edgion_resources::resources::refs::acme_fields::AcmeRefs;
/// fn positive(owner: &edgion_resources::EdgionAcme) {
///     let refs = AcmeRefs::from_acme(owner);
///     let _ = refs.account().token().validated();
/// }
/// ```
/// ```compile_fail
/// use edgion_resources::resources::refs::acme_fields::AcmeRefs;
/// fn invalid(owner: edgion_resources::EdgionAcme) {
///     let refs = AcmeRefs::from_acme(&owner);
///     let field = refs.account();
///     drop(owner);
///     let _ = field.token().validated();
/// }
/// ```
/// ```compile_fail
/// use edgion_resources::resources::refs::acme_fields::AcmeRefs;
/// fn invalid(owner: &mut edgion_resources::EdgionAcme) {
///     let refs = AcmeRefs::from_acme(owner);
///     let field = refs.account();
///     owner.spec.external_account_binding = None;
///     let _ = field.token().validated();
/// }
/// ```
/// ```compile_fail
/// use edgion_resources::resources::refs::acme_fields::AcmeRefs;
/// fn invalid(owner: &mut edgion_resources::EdgionAcme) {
///     let refs = AcmeRefs::from_acme(owner);
///     let field = refs.account();
///     owner.spec.private_key_secret_ref.name.clear();
///     let _ = field.token().validated();
/// }
/// ```
/// ```compile_fail
/// use edgion_resources::resources::refs::acme_fields::{AcmeRefs, AcmeAccountField};
/// fn persistence(_: &AcmeAccountField<'_>) {}
/// fn invalid(owner: &edgion_resources::EdgionAcme) {
///     let refs = AcmeRefs::from_acme(owner);
///     persistence(&refs.eab().unwrap());
/// }
/// ```
pub struct AcmeRefs<'a> {
    acme: &'a EdgionAcme,
    owner: OwnerSnapshot,
    account: PreparedSlot,
    eab: Option<PreparedSlot>,
}
pub struct AcmeAccountField<'a> {
    refs: &'a AcmeRefs<'a>,
}
pub struct AcmeEabField<'a> {
    refs: &'a AcmeRefs<'a>,
    slot: &'a PreparedSlot,
}
impl<'a> AcmeRefs<'a> {
    pub fn from_acme(acme: &'a EdgionAcme) -> Self {
        let owner = OwnerSnapshot::namespaced("edgion.io", "EdgionAcme", &acme.metadata);
        let account = PreparedSlot::new(
            &owner,
            FieldWire::Secret(acme.spec.private_key_secret_ref.clone()),
            FieldContract::StrictSecret,
            FieldSlot::AcmeAccount,
        );
        let eab = acme.spec.external_account_binding.as_ref().map(|eab| {
            PreparedSlot::new(
                &owner,
                FieldWire::Secret(eab.key_secret_ref.clone()),
                FieldContract::StrictSecret,
                FieldSlot::AcmeEab,
            )
        });
        Self {
            acme,
            owner,
            account,
            eab,
        }
    }
    pub fn account(&self) -> AcmeAccountField<'_> {
        AcmeAccountField { refs: self }
    }
    pub fn eab(&self) -> Option<AcmeEabField<'_>> {
        self.eab.as_ref().map(|slot| AcmeEabField { refs: self, slot })
    }
}
impl AcmeAccountField<'_> {
    pub fn token(&self) -> ReferenceToken<'_> {
        self.refs.account.token(TopologyLoan::Acme(&self.refs.owner))
    }
    pub fn owner(&self) -> &EdgionAcme {
        self.refs.acme
    }
    pub fn validate_api_secret(&self, secret: &Secret) -> Result<(), ReferenceError> {
        validate_api_secret(self.token(), secret)
    }
}
impl AcmeEabField<'_> {
    pub fn token(&self) -> ReferenceToken<'_> {
        self.slot.token(TopologyLoan::Acme(&self.refs.owner))
    }
    pub fn validate_api_secret(&self, secret: &Secret) -> Result<(), ReferenceError> {
        validate_api_secret(self.token(), secret)
    }
}
// Kubernetes API responses carry an explicit namespace. Raw cache default aliases
// belong to the finite acquisition selector, not this API-response boundary.
fn validate_api_secret(token: ReferenceToken<'_>, secret: &Secret) -> Result<(), ReferenceError> {
    let expected = token.validated()?.target();
    let name = secret.metadata.name.as_deref().ok_or(ReferenceError::StaleBinding)?;
    let namespace = secret
        .metadata
        .namespace
        .as_deref()
        .ok_or(ReferenceError::StaleBinding)?;
    let actual = ObjectIdentity::checked("", "Secret", ResourceScope::Namespaced(namespace), name)?;
    if actual == expected {
        Ok(())
    } else {
        Err(ReferenceError::StaleBinding)
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/acme_fields_test.rs"]
mod tests;
