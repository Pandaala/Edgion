//! Common types shared across LinkSys configurations

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Reference to a Kubernetes Secret
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SecretReference {
    /// Name of the secret
    pub name: String,

    /// Namespace of the secret (defaults to LinkSys namespace)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,

    /// Key in the secret for username
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username_key: Option<String>,

    /// Key in the secret for password
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_key: Option<String>,
}
