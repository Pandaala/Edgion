//! Same-namespace policy targets borrowed from their actual owners.
use crate::resources::edgion_backend_traffic_policy::PolicyTargetKey;
use crate::{BackendTLSPolicy, EdgionBackendTrafficPolicy};

/// Native policy fields retain the real owner while resolving its namespace.
/// ```compile_fail
/// use edgion_resources::resources::refs::policy_fields::BackendTlsPolicyTargets;
/// fn release(policy: edgion_resources::BackendTLSPolicy) {
///     let field = BackendTlsPolicyTargets::from_policy(&policy).fields().next().unwrap();
///     drop(policy);
///     field.canonical_key();
/// }
/// ```
#[derive(Clone, Copy)]
pub struct BackendTlsPolicyTargets<'a> {
    owner: &'a BackendTLSPolicy,
}
impl<'a> BackendTlsPolicyTargets<'a> {
    pub fn from_policy(owner: &'a BackendTLSPolicy) -> Self {
        Self { owner }
    }
    pub fn fields(self) -> impl Iterator<Item = BackendTlsPolicyTarget<'a>> {
        (0..self.owner.spec.target_refs.len()).map(move |index| BackendTlsPolicyTarget {
            owner: self.owner,
            index,
        })
    }
}
pub struct BackendTlsPolicyTarget<'a> {
    owner: &'a BackendTLSPolicy,
    index: usize,
}
impl BackendTlsPolicyTarget<'_> {
    pub fn canonical_key(&self) -> Result<PolicyTargetKey, String> {
        self.owner.spec.target_refs[self.index]
            .canonical_key(self.owner.metadata.namespace.as_deref().unwrap_or("default"))
    }
    pub fn section_name(&self) -> Option<&str> {
        self.owner.spec.target_refs[self.index].section_name.as_deref()
    }
}

/// Custom policy targets keep the finite custom group aliases of their field.
/// ```compile_fail
/// use edgion_resources::resources::refs::policy_fields::BackendTrafficPolicyTargets;
/// fn release(policy: edgion_resources::EdgionBackendTrafficPolicy) {
///     let field = BackendTrafficPolicyTargets::from_policy(&policy).fields().next().unwrap();
///     drop(policy);
///     field.canonical_key();
/// }
/// ```
#[derive(Clone, Copy)]
pub struct BackendTrafficPolicyTargets<'a> {
    owner: &'a EdgionBackendTrafficPolicy,
}
impl<'a> BackendTrafficPolicyTargets<'a> {
    pub fn from_policy(owner: &'a EdgionBackendTrafficPolicy) -> Self {
        Self { owner }
    }
    pub fn fields(self) -> impl Iterator<Item = BackendTrafficPolicyTarget<'a>> {
        (0..self.owner.spec.target_refs.len()).map(move |index| BackendTrafficPolicyTarget {
            owner: self.owner,
            index,
        })
    }
}
pub struct BackendTrafficPolicyTarget<'a> {
    owner: &'a EdgionBackendTrafficPolicy,
    index: usize,
}
impl BackendTrafficPolicyTarget<'_> {
    pub fn canonical_key(&self) -> Result<PolicyTargetKey, String> {
        self.owner.spec.target_refs[self.index]
            .canonical_key(self.owner.metadata.namespace.as_deref().unwrap_or("default"))
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/policy_fields_test.rs"]
mod tests;
