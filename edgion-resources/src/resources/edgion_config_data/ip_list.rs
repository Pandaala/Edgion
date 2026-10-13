//! IP list data
//!
//! A grouped list of IP addresses or CIDR ranges for IP-based filtering.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::edgion_config_data::IpGroup;

/// A grouped list of IP addresses or CIDR ranges.
///
/// `items` is required; an empty array is a valid, present empty list.
/// Each named `IpGroup` supplies CIDRs. Central partial-good compilation
/// excludes invalid entries and preserves valid siblings; longest-prefix
/// matching selects the group within this list.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IpListData {
    pub items: Vec<IpGroup>,
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_config_data/ip_list_test.rs"]
mod tests;
