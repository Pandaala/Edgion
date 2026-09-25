//! Named WAF policy inputs with optional content pins, selected by a consuming plugin.

use super::EdgionConfigDataRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MAX_WAF_BUNDLE_REFS: usize = 16;
const MAX_WAF_TOTAL_REFS: usize = 64;

/// One shared rule bundle with an optional content pin.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WafBundleRef {
    #[serde(flatten)]
    pub target: EdgionConfigDataRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_digest: Option<String>,
    /// An independent addition that may be omitted when unavailable or invalid.
    #[serde(default)]
    pub optional: bool,
}

/// Ordered rule inputs for one selectable WAF policy.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WafProfile {
    pub bundle_refs: Vec<WafBundleRef>,
}

/// A shared WAF policy containing named combinations of rule bundles.
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WafPolicyData {
    pub profiles: BTreeMap<String, WafProfile>,
    pub default_profile: String,
}

impl WafPolicyData {
    /// Validate bounded profile inputs before resolving referenced resources.
    pub fn validate_shape(&self) -> Result<(), String> {
        if self.profiles.is_empty() || self.profiles.len() > 16 {
            return Err("profiles must contain between 1 and 16 entries".to_string());
        }
        if !self.profiles.contains_key(&self.default_profile) {
            return Err("defaultProfile must name a configured profile".to_string());
        }
        let mut total_refs = 0_usize;
        let mut unique_refs = std::collections::HashSet::new();
        for (name, profile) in &self.profiles {
            if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
                return Err("profiles contains an invalid name".to_string());
            }
            if profile.bundle_refs.is_empty() || profile.bundle_refs.len() > MAX_WAF_BUNDLE_REFS {
                return Err(format!("each profile requires 1 to {MAX_WAF_BUNDLE_REFS} bundleRefs"));
            }
            if !profile.bundle_refs.iter().any(|reference| !reference.optional) {
                return Err("each profile requires a non-optional bundleRef".to_string());
            }
            total_refs += profile.bundle_refs.len();
            for reference in &profile.bundle_refs {
                unique_refs.insert((reference.target.namespace.as_deref(), reference.target.name.as_str()));
                if reference.target.name.is_empty() || reference.target.name.contains('/') {
                    return Err("bundleRefs contains an invalid name".to_string());
                }
                if let Some(digest) = &reference.expected_digest {
                    if digest.len() != 64
                        || !digest
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                    {
                        return Err("bundleRefs expectedDigest must be a lowercase SHA-256 digest".to_string());
                    }
                }
            }
        }
        if total_refs > MAX_WAF_TOTAL_REFS || unique_refs.len() > MAX_WAF_BUNDLE_REFS {
            return Err("profiles exceed the total or unique bundle reference limit".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_config_data/waf_policy_test.rs"]
mod tests;
