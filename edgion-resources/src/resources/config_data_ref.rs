//! Leaf reference type for `EdgionConfigData`.
//!
//! Lives outside `edgion_config_data/mod.rs` so `Condition` can name
//! `EdgionConfigDataRef` without compiling `request_access_url_allow_list`
//! (that module imports `plugin_configs`, which imports `conditions`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Reference to an EdgionConfigData resource by name (and optional namespace).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EdgionConfigDataRef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Controller-set cross-namespace denial marker. When `Some`, this ref lacks a
    /// permitting `ReferenceGrant` (and the target is not `visibility: Cluster`); the
    /// gateway withholds its matcher (deny-all fail-closed for allowRefs). Not
    /// operator-facing — excluded from the CRD via `schemars(skip)`; serialized for
    /// conf_sync so the gateway sees it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub ref_denied: Option<crate::resources::common::RefDenied>,
}
