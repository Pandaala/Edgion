//! Ordered reference-only ACL policy inputs and bounded Controller read proofs.
use super::ip_reference_set::IpReadResolution;
use crate::matcher::ip_cidr_helpers::validate_refs_structure;
use crate::resources::edgion_config_data::EdgionConfigDataRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IpAclReferenceSet {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_refs: Option<Vec<EdgionConfigDataRef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deny_refs: Option<Vec<EdgionConfigDataRef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_allow_refs: Option<Box<IpReadResolution>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_deny_refs: Option<Box<IpReadResolution>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[schemars(skip)]
    pub invalid_input: bool,
}
/// A transient cold wire shadow. Rejected values are never retained or transported.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IpAclWire {
    #[serde(default)]
    allow_refs: Option<Vec<EdgionConfigDataRef>>,
    #[serde(default)]
    deny_refs: Option<Vec<EdgionConfigDataRef>>,
    #[serde(default)]
    resolved_allow_refs: Option<Box<IpReadResolution>>,
    #[serde(default)]
    resolved_deny_refs: Option<Box<IpReadResolution>>,
    #[serde(default)]
    invalid_input: bool,
    #[serde(flatten)]
    unknown: HashMap<String, serde::de::IgnoredAny>,
}
impl IpAclWire {
    pub(crate) fn contains(&self, field: &str) -> bool {
        self.unknown.contains_key(field)
    }
    pub(crate) fn into_set(self) -> IpAclReferenceSet {
        IpAclReferenceSet {
            invalid_input: self.invalid_input || self.contains("allow") || self.contains("deny"),
            allow_refs: self.allow_refs,
            deny_refs: self.deny_refs,
            resolved_allow_refs: self.resolved_allow_refs,
            resolved_deny_refs: self.resolved_deny_refs,
        }
    }
}
impl<'de> Deserialize<'de> for IpAclReferenceSet {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(IpAclWire::deserialize(deserializer)?.into_set())
    }
}
impl IpAclReferenceSet {
    pub fn allow_mode(&self) -> bool {
        self.allow_refs.as_ref().is_some_and(|refs| !refs.is_empty())
    }
    pub fn validation_error(&self) -> Option<String> {
        if self.invalid_input {
            return Some("Inline ACL allow/deny fields are not supported; use allowRefs/denyRefs".into());
        }
        if !self.allow_mode() && self.deny_refs.as_ref().is_none_or(|refs| refs.is_empty()) {
            return Some("At least one of 'allowRefs'/'denyRefs' must be non-empty".into());
        }
        for (name, refs) in [("allowRefs", &self.allow_refs), ("denyRefs", &self.deny_refs)] {
            if let Some(refs) = refs {
                if let Err(error) = validate_refs_structure(refs) {
                    return Some(format!("{name}: {error}"));
                }
            }
        }
        None
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_plugins/ip_acl_refs_test.rs"]
mod tests;
