//! GCIR selects git-owned named policies whose collections are external IpLists.
//! A Selector can only choose among validated policies; errors fall back to the base.
use crate::resources::edgion_config_data::EdgionConfigDataRef;
use crate::resources::edgion_plugins::ip_acl_refs::{IpAclReferenceSet, IpAclWire};
use crate::resources::edgion_plugins::{DefaultAction, PluginValidation};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
/// Choose a conservative base policy: Selector failures use `active_profile`.
/// Missing deny references are omitted, so their default action remains significant.
pub struct GlobalConnectionIpRestrictionConfig {
    /// Explicit emergency switch; false allows connections without list reads.
    pub enable: bool,
    /// Base profile used when the genuine Selector is unavailable or invalid.
    pub active_profile: String,
    /// Named scalar policies; each IP collection lives in its referenced IpList.
    pub profiles: HashMap<String, ProfileRules>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile_ref: Option<EdgionConfigDataRef>,
}
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRules {
    #[serde(flatten)]
    pub refs: IpAclReferenceSet,
    #[serde(default)]
    pub default_action: DefaultAction,
}
impl<'de> Deserialize<'de> for ProfileRules {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            #[serde(default)]
            default_action: DefaultAction,
            #[serde(flatten)]
            refs: IpAclWire,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            refs: wire.refs.into_set(),
            default_action: wire.default_action,
        })
    }
}
impl PluginValidation for ProfileRules {
    fn validation_error(&self) -> Option<String> {
        self.refs.validation_error()
    }
}
impl PluginValidation for GlobalConnectionIpRestrictionConfig {
    fn validation_error(&self) -> Option<String> {
        if self.profiles.is_empty() {
            return Some("GlobalConnectionIpRestriction requires at least one profile".into());
        }
        let mut names: Vec<_> = self.profiles.keys().collect();
        names.sort();
        if !self.profiles.contains_key(&self.active_profile) {
            return Some(format!(
                "activeProfile '{}' not found in profiles {:?}",
                self.active_profile, names
            ));
        }
        for name in names {
            if let Some(error) = self.profiles[name].validation_error() {
                return Some(format!("profile '{name}': {error}"));
            }
        }
        None
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_stream_plugins/global_connection_ip_restriction_test.rs"]
mod tests;
