//! Shared IP group primitive.
//!
//! `IpGroup` owns named CIDR collections exclusively within `IpList`. Business
//! consumers reference these lists and borrow the Gateway's central compiled
//! matchers; they never copy or recompile the groups in plugin configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A named group of CIDR ranges / bare IPs.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IpGroup {
    /// Group name, unique within one IpList; retained for canonical match attribution.
    pub name: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// CIDR strings or bare IPs (bare IPs are internally converted to /32 or /128).
    pub cidrs: Vec<String>,
}
