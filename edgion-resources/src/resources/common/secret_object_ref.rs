//! Generic Kubernetes object reference for Secret-bearing CRD fields.
//!
//! Owning fields interpret omitted group as the empty Kubernetes core group and
//! omitted kind as `Secret`. Literal `core` is accepted only by the finite Edgion
//! custom-field contracts, not as a native Gateway API or ACME default.
//! CredentialInjector also supports exact `edgion.io/LinkSys`; other owning
//! fields retain their own supported targets and namespace/authorization policy.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq, Hash, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretObjectReference {
    /// Group is the group of the referent. Defaults to `""` (core).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,

    /// Kind is kind of the referent. Defaults to `Secret`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,

    /// Name is the name of the referent.
    pub name: String,

    /// Namespace is the namespace of the referent.
    /// Defaulting and cross-namespace authorization follow the owning field contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

impl SecretObjectReference {
    /// Resolve the target namespace (falls back to the owner namespace when absent)
    pub fn effective_namespace<'a>(&'a self, owner_ns: &'a str) -> &'a str {
        self.namespace.as_deref().unwrap_or(owner_ns)
    }

    /// Whether this is a cross-namespace reference
    pub fn is_cross_namespace(&self, owner_ns: &str) -> bool {
        match self.namespace.as_deref() {
            Some(ns) => ns != owner_ns,
            None => false,
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/common/secret_object_ref_test.rs"]
mod tests;
