//! Reusable "Secret reference + resolved Secret" slot for CRD fields.
//!
//! Embedded into `*Auth` structs / enum variants and `TlsCerts` via
//! `#[serde(flatten)]` so the YAML form is unchanged: `secretRef` appears at
//! the parent's top level.
//!
//! The `secret` field is **never** populated from user YAML — it's filled by
//! the Controller during Secret resolution and serialized to the Gateway via
//! conf_sync. Operators never write it.

use k8s_openapi::api::core::v1::Secret;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::SecretObjectReference;

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretSlot {
    /// Reference to the Kubernetes Secret carrying credential / TLS material.
    pub secret_ref: SecretObjectReference,

    /// Resolved Secret object (filled by Controller, never from user YAML).
    /// Serialized to the Gateway via conf_sync; not persisted into K8s `spec`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub secret: Option<Secret>,
}

#[cfg(test)]
mod tests {
    use super::SecretSlot;

    #[test]
    fn deserialize_secret_ref_only() {
        let yaml = "secretRef:\n  name: creds";
        let slot: SecretSlot = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(slot.secret_ref.name, "creds");
        assert!(slot.secret.is_none());
    }

    #[test]
    fn rejects_unknown_field() {
        let yaml = "secretRef:\n  name: creds\nbogus: 1";
        assert!(serde_yaml::from_str::<SecretSlot>(yaml).is_err());
    }

    #[test]
    fn secret_ref_required() {
        let yaml = "{}";
        assert!(serde_yaml::from_str::<SecretSlot>(yaml).is_err());
    }
}
