//! Shared IP group primitive.
//!
//! `IpGroup` is a named bundle of CIDR strings used by multiple EdgionConfigData
//! variants and plugin configs (e.g. `IpList`, and the GlobalConnectionIpRestriction
//! stream-plugin profiles). It lives here, in the `edgion_config_data` primitive
//! module, so both the config-data variants and the stream/HTTP plugins can share it
//! without a cross-module ownership tangle.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A named group of CIDR ranges / bare IPs.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IpGroup {
    /// Group name (unique within a profile). Used for admin organization; runtime flattens all groups.
    pub name: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// CIDR strings or bare IPs (bare IPs are internally converted to /32 or /128).
    pub cidrs: Vec<String>,
}
