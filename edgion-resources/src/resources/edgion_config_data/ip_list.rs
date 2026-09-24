//! IP list data
//!
//! A grouped list of IP addresses or CIDR ranges for IP-based filtering.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::edgion_config_data::IpGroup;

/// A grouped list of IP addresses or CIDR ranges.
///
/// `items` is required and must be non-empty. Each `IpGroup` carries its
/// own name (for log/observability) and `cidrs` (non-empty). Groups are
/// merged at runtime into a single `IpRadixMatcher`; longest-prefix match
/// selects the matched group when subnets overlap across groups.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IpListData {
    pub items: Vec<IpGroup>,
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_config_data/ip_list_test.rs"]
mod tests;
