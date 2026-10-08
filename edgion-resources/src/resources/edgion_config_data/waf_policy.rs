//! Named WAF policy inputs with optional content pins, selected by a consuming plugin.

use super::EdgionConfigDataRef;
use edgion_waf::seclang::{PreparedWafRuleSelection, MAX_POLICY_SELECTION_BYTES};
pub use edgion_waf::seclang::{WafRuleIdRange, WafRuleSelection, WafRuleSelectionExpr, WafSelectionSummary};
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
    /// Static complete-rule-unit selection, evaluated only during compilation.
    #[serde(default, skip_serializing_if = "WafRuleSelection::is_empty")]
    pub rule_selection: WafRuleSelection,
}

/// Invocation-scoped selection programs; never serialized or used as admission certificates.
#[derive(Debug)]
pub struct PreparedWafPolicySelections {
    profiles: Vec<(String, PreparedWafRuleSelection)>,
    live_bytes: usize,
}

impl PreparedWafPolicySelections {
    pub fn selection(&self, name: &str, profile: &WafProfile) -> Result<Option<&PreparedWafRuleSelection>, String> {
        let program = self
            .profiles
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, program)| program);
        match program {
            Some(program) if program.matches_selection(&profile.rule_selection) => {
                Ok((!program.is_empty()).then_some(program))
            }
            None if self.profiles.is_empty() && profile.rule_selection.is_empty() => Ok(None),
            _ => Err("WAF selection program does not match the selected profile".to_string()),
        }
    }
    /// Complete policy tree and all prepared programs alive during each compile pass.
    pub const fn live_bytes(&self) -> usize {
        self.live_bytes
    }

    pub fn validate_binding(&self, policy: &WafPolicyData) -> Result<(), String> {
        let matches = if self.profiles.is_empty() {
            policy
                .profiles
                .values()
                .all(|profile| profile.rule_selection.is_empty())
        } else {
            self.profiles.len() == policy.profiles.len()
                && self.profiles.iter().all(|(name, program)| {
                    policy
                        .profiles
                        .get(name)
                        .is_some_and(|profile| program.matches_selection(&profile.rule_selection))
                })
        };
        if matches {
            Ok(())
        } else {
            Err("WAF selection programs do not match the policy profiles".to_string())
        }
    }
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
        let mut selection_bytes = 0_usize;
        let mut unique_refs = std::collections::HashSet::new();
        for (name, profile) in &self.profiles {
            selection_bytes = selection_bytes
                .checked_add(
                    profile
                        .rule_selection
                        .validate()
                        .map_err(|e| e.to_string())?
                        .encoded_bytes,
                )
                .ok_or("WAF policy selection size overflow")?;
            if selection_bytes > MAX_POLICY_SELECTION_BYTES {
                return Err("WAF policy selections exceed 64 KiB".to_string());
            }
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

    /// Prepare every profile once before referenced sources are captured.
    pub fn prepare_selections(&self) -> Result<PreparedWafPolicySelections, String> {
        self.validate_shape()?;
        let mut owner = PreparedWafPolicySelections {
            profiles: Vec::new(),
            live_bytes: 0,
        };
        if self.profiles.values().all(|profile| profile.rule_selection.is_empty()) {
            return Ok(owner);
        }
        owner
            .profiles
            .try_reserve_exact(self.profiles.len())
            .map_err(|_| "cannot allocate WAF policy selections")?;
        owner.live_bytes = std::mem::size_of::<PreparedWafPolicySelections>()
            + owner.profiles.capacity() * std::mem::size_of::<(String, PreparedWafRuleSelection)>();
        for (name, profile) in &self.profiles {
            let mut key = String::new();
            key.try_reserve_exact(name.len())
                .map_err(|_| "cannot allocate WAF profile name")?;
            key.push_str(name);
            let program = profile.rule_selection.prepare().map_err(|e| e.to_string())?;
            // Both typed policy inputs and sealed programs remain live across trials.
            owner.live_bytes = owner
                .live_bytes
                .checked_add(key.capacity())
                .and_then(|bytes| bytes.checked_add(name.capacity()))
                .and_then(|bytes| bytes.checked_add(profile.rule_selection.validate().ok()?.owned_bytes))
                .and_then(|bytes| {
                    bytes.checked_add(program.owned_bytes() - std::mem::size_of::<PreparedWafRuleSelection>())
                })
                .ok_or("WAF selection memory accounting overflow")?;
            owner.profiles.push((key, program));
        }
        Ok(owner)
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_config_data/waf_policy_test.rs"]
mod tests;
