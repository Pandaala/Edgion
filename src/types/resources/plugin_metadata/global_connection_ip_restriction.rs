//! GlobalConnectionIpRestriction PluginMetaData variant — L4 TCP connection-phase IP restriction.

// Note: `build_radix_matcher` and `IpAddr` are unused in this task (Task 3, types-only).
// They will be used in Task 4 when `validate_and_init` and `check_ip` are implemented.
// Using `#[allow(unused_imports)]` here so the import is pre-staged and ready for Task 4.
#[allow(unused_imports)]
use crate::core::common::matcher::ip_cidr_helpers::build_radix_matcher;
#[allow(unused_imports)]
use std::net::IpAddr;

use crate::core::common::matcher::ip_radix_tree::IpRadixMatcher;
use crate::types::resources::edgion_plugins::DefaultAction;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ===== PM data root =====

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GlobalConnectionIpRestrictionData {
    /// Emergency kill switch. When `false`, the data plane completely skips this PM
    /// (equivalent to not referenced).
    pub enable: bool,

    /// Currently active profile key. Must exist in `profiles`.
    pub active_profile: String,

    /// Named rule presets. Switch `active_profile` to swap rules instantly.
    pub profiles: HashMap<String, ProfileRules>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRules {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<IpGroup>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub deny: Option<Vec<IpGroup>>,

    #[serde(default)]
    pub default_action: DefaultAction,

    // Runtime-only, not serialized
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) allow_matcher: Option<IpRadixMatcher>,
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) deny_matcher: Option<IpRadixMatcher>,
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_profile(name: &str) -> (String, ProfileRules) {
        (
            name.to_string(),
            ProfileRules {
                allow: Some(vec![IpGroup {
                    name: "office".to_string(),
                    description: None,
                    cidrs: vec!["192.168.1.0/24".to_string()],
                }]),
                deny: None,
                default_action: DefaultAction::Deny,
                allow_matcher: None,
                deny_matcher: None,
            },
        )
    }

    #[test]
    fn construction_and_serde_roundtrip() {
        let (k, v) = valid_profile("p1");
        let data = GlobalConnectionIpRestrictionData {
            enable: true,
            active_profile: "p1".to_string(),
            profiles: HashMap::from([(k, v)]),
            description: Some("test".to_string()),
        };
        let j = serde_json::to_value(&data).unwrap();
        let back: GlobalConnectionIpRestrictionData = serde_json::from_value(j).unwrap();
        assert_eq!(back.active_profile, "p1");
        assert!(back.enable);
    }
}
