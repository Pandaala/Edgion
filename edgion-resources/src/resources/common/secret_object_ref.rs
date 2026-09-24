//! Generic Kubernetes object reference for Secret-bearing CRD fields.
//!
//! Schema follows Gateway API `SecretObjectReference` (group/kind defaulted to
//! `core/Secret`). The `group` and `kind` slots are reserved for future
//! non-Secret credential sources (Vault CRD, ExternalSecret, ...).

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
    /// Cross-namespace refs require a Gateway API `ReferenceGrant`.
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
