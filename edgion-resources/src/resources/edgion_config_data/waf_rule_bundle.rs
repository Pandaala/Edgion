//! Strict rule inputs with an optional content self-check for a native WAF generation.

use std::collections::HashSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_MEMBERS: usize = 1_024;
const MAX_MEMBER_BYTES: usize = 16 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;
const MAX_NAME_BYTES: usize = 4 * 1024;
const MAX_METADATA_BYTES: usize = 4 * 1024;

/// A named rule file or phrase asset within one WAF bundle.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WafBundleMember {
    pub name: String,
    #[serde(serialize_with = "crate::redact::redact_required::serialize")]
    pub content: String,
}

impl std::fmt::Debug for WafBundleMember {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WafBundleMember")
            .field("name", &self.name)
            .field("content", &"[redacted]")
            .finish()
    }
}

/// One versioned rule bundle. The consumer compiles selected bundles together;
/// standalone validation proves this bundle's structure and any supplied digest.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WafRuleBundleData {
    /// Operator-selected version, not an implicit update channel.
    pub version: String,
    /// Profile identifier describing this bundle's intended rule set.
    pub profile: String,
    /// Human-readable origin such as a repository and revision.
    pub provenance: String,
    /// Optional self-check of the canonical bundle contents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// Ordered names of rule members selected as entry points.
    pub roots: Vec<String>,
    /// Rule files, including files reached by Include.
    pub rules: Vec<WafBundleMember>,
    /// Named text files used by @pmFromFile.
    #[serde(default)]
    pub phrase_assets: Vec<WafBundleMember>,
}

impl std::fmt::Debug for WafRuleBundleData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WafRuleBundleData")
            .field("version", &self.version)
            .field("profile", &self.profile)
            .field("provenance", &self.provenance)
            .field("digest", &self.digest)
            .field("roots", &self.roots)
            .field("rules", &self.rules)
            .field("phrase_assets", &self.phrase_assets)
            .finish()
    }
}

impl WafRuleBundleData {
    /// Compute a canonical digest independent of member declaration order.
    /// Root order remains significant because it determines rule order.
    pub fn compute_digest(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(b"edgion-waf-rule-bundle-v1\0");
        update_field(&mut hash, self.version.as_bytes());
        update_field(&mut hash, self.profile.as_bytes());
        update_field(&mut hash, self.provenance.as_bytes());
        update_names(&mut hash, &self.roots);
        update_members(&mut hash, &self.rules);
        update_members(&mut hash, &self.phrase_assets);
        hex::encode(hash.finalize())
    }

    /// Reject an incomplete or tampered bundle before a consumer can bind it.
    pub fn validate(&self) -> Result<(), String> {
        for (label, value) in [
            ("version", &self.version),
            ("profile", &self.profile),
            ("provenance", &self.provenance),
        ] {
            if value.trim().is_empty() || value.len() > MAX_METADATA_BYTES || value.chars().any(char::is_control) {
                return Err(format!(
                    "WafRuleBundle {label} is empty, too long, or contains control characters"
                ));
            }
        }
        if self.roots.is_empty() || self.rules.is_empty() {
            return Err("WafRuleBundle requires at least one root and one rule member".into());
        }
        if self.roots.len() > MAX_MEMBERS
            || self.rules.len() > MAX_MEMBERS
            || self.phrase_assets.len() > MAX_MEMBERS
            || self.rules.len() + self.phrase_assets.len() > MAX_MEMBERS
        {
            return Err("WafRuleBundle member count exceeds the limit".into());
        }

        let mut rule_names = HashSet::new();
        let mut all_names = HashSet::new();
        let mut total_bytes = 0usize;
        for member in self.rules.iter().chain(&self.phrase_assets) {
            if !safe_relative_name(&member.name) || !all_names.insert(member.name.as_str()) {
                return Err("WafRuleBundle has an unsafe or duplicate member name".into());
            }
            if member.content.len() > MAX_MEMBER_BYTES {
                return Err("WafRuleBundle member exceeds the byte limit".into());
            }
            total_bytes = total_bytes.saturating_add(member.content.len());
            if total_bytes > MAX_TOTAL_BYTES {
                return Err("WafRuleBundle exceeds the aggregate byte limit".into());
            }
        }
        for member in &self.rules {
            rule_names.insert(member.name.as_str());
        }
        let mut selected_roots = HashSet::new();
        for root in &self.roots {
            if !safe_relative_name(root) || !selected_roots.insert(root.as_str()) || !rule_names.contains(root.as_str())
            {
                return Err("WafRuleBundle has an unsafe, duplicate, or missing root".into());
            }
        }
        if let Some(digest) = &self.digest {
            if digest.len() != 64
                || !digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                || digest != &self.compute_digest()
            {
                return Err("WafRuleBundle digest does not match its contents".into());
            }
        }
        Ok(())
    }
}

fn safe_relative_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-'))
        && name
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn update_field(hash: &mut Sha256, field: &[u8]) {
    hash.update((field.len() as u64).to_be_bytes());
    hash.update(field);
}

fn update_names(hash: &mut Sha256, names: &[String]) {
    hash.update((names.len() as u64).to_be_bytes());
    for name in names {
        update_field(hash, name.as_bytes());
    }
}

fn update_members(hash: &mut Sha256, members: &[WafBundleMember]) {
    let mut sorted: Vec<_> = members.iter().collect();
    sorted.sort_unstable_by(|left, right| left.name.cmp(&right.name));
    hash.update((sorted.len() as u64).to_be_bytes());
    for member in sorted {
        update_field(hash, member.name.as_bytes());
        update_field(hash, member.content.as_bytes());
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_config_data/waf_rule_bundle_test.rs"]
mod tests;
