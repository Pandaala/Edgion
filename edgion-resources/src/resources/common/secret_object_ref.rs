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
mod tests {
    use super::SecretObjectReference;

    #[test]
    fn deserialize_minimal() {
        let yaml = "name: my-secret";
        let s: SecretObjectReference = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(s.name, "my-secret");
        assert_eq!(s.namespace, None);
        assert_eq!(s.group, None);
        assert_eq!(s.kind, None);
    }

    #[test]
    fn deserialize_with_group_kind() {
        let yaml = "name: my-secret\ngroup: \"\"\nkind: Secret\nnamespace: credentials";
        let s: SecretObjectReference = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(s.group.as_deref(), Some(""));
        assert_eq!(s.kind.as_deref(), Some("Secret"));
        assert_eq!(s.namespace.as_deref(), Some("credentials"));
    }

    #[test]
    fn name_required() {
        let yaml = "namespace: credentials";
        assert!(serde_yaml::from_str::<SecretObjectReference>(yaml).is_err());
    }

    #[test]
    fn rejects_unknown_fields() {
        let yaml = "name: my-secret\nunknownField: forbidden";
        assert!(serde_yaml::from_str::<SecretObjectReference>(yaml).is_err());
    }

    #[test]
    fn effective_namespace_falls_back() {
        let s = SecretObjectReference {
            group: None,
            kind: None,
            name: "s".into(),
            namespace: None,
        };
        assert_eq!(s.effective_namespace("owner-ns"), "owner-ns");
    }
}
