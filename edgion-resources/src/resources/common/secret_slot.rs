//! Reusable "Secret reference + resolved Secret" slot for CRD fields.
//!
//! Embedded into `*Auth` structs / enum variants via `#[serde(flatten)]` so
//! the YAML form is unchanged: `secretRef` appears at the parent's top level.
//!
//! The `secret` field is **never** populated from user YAML — it's filled by
//! the Controller during Secret resolution and serialized to the Gateway via
//! conf_sync. Operators never write it.

use k8s_openapi::api::core::v1::Secret;

crate::redact::impl_redacted_debug!(SecretSlot);
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::SecretObjectReference;

#[derive(Deserialize, Serialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretSlot {
    /// Reference to the Kubernetes Secret carrying credential / TLS material.
    pub secret_ref: SecretObjectReference,

    /// Resolved Secret object (filled by Controller, never from user YAML).
    /// Serialized to the Gateway via conf_sync; not persisted into K8s `spec`.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub secret: Option<Secret>,
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/common/secret_slot_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests/unit/resources/common/secret_slot_test/redact_tests.rs"]
mod redact_tests;
