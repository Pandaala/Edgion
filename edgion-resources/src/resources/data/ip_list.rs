//! IP list data
//!
//! A grouped list of IP addresses or CIDR ranges for IP-based filtering.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::data::IpGroup;

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
mod tests {
    use super::*;
    use crate::matcher::ip_cidr_helpers::validate_groups_structure;

    #[test]
    fn empty_items_validation_errs() {
        let data = IpListData { items: vec![] };
        assert!(validate_groups_structure(&data.items).is_err());
    }

    #[test]
    fn single_group_valid() {
        let data = IpListData {
            items: vec![IpGroup {
                name: "default".into(),
                description: None,
                cidrs: vec!["10.0.0.0/8".into()],
            }],
        };
        assert!(validate_groups_structure(&data.items).is_ok());
    }
}
