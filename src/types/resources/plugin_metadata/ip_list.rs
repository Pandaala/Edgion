//! IP list metadata
//!
//! A list of IP addresses or CIDR ranges for IP-based filtering.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A list of IP addresses or CIDR ranges.
///
/// Used by plugins like IpRestriction to reference external IP allowlists
/// or blocklists by PluginMetaData name.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IpListData {
    /// IP addresses or CIDR ranges (e.g. "192.168.1.0/24", "10.0.0.0/8", "2001:db8::/32")
    pub items: Vec<String>,
}
