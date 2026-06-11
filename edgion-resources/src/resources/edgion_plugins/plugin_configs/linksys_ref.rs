use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LinkSysRef {
    /// Kubernetes namespace of the LinkSys resource.
    pub namespace: String,
    /// Name of the LinkSys resource. Also used as the alias in DSL scripts.
    /// Must be unique within a single linkSysRefs list.
    /// Allowed characters: a-z 0-9 - _
    pub name: String,
}
