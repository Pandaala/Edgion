//! Reference-only IP collection data and bounded Controller read metadata.
use crate::resources::edgion_config_data::EdgionConfigDataRef;
use schemars::JsonSchema;
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IpReadScope {
    SameNamespace,
    ClusterOnly,
    GrantAuthorized,
    ValidationDisabled,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpReadSource {
    pub group: String,
    pub kind: String,
    pub namespace: Option<String>,
    pub name: Option<String>,
    pub uid: Option<String>,
    pub generation: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpReadSlot {
    pub namespace: Option<String>,
    pub name: Option<String>,
    pub scope: IpReadScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IpReadDomain {
    Condition,
    IpList,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpReadResolution {
    pub domain: IpReadDomain,
    pub source: IpReadSource,
    pub root: usize,
    pub leaf: Option<(usize, Option<usize>)>,
    pub slots: Vec<IpReadSlot>,
}

/// Authored references, with no inline addresses or consumer-owned matcher.
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IpReferenceSet {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ip_refs: Vec<EdgionConfigDataRef>,
    /// Monotone invalidity survives transport without retaining rejected values.
    #[serde(default, skip_serializing_if = "is_false")]
    #[schemars(skip)]
    pub invalid_input: bool,
    /// Recomputed by the Controller from actual owning field loans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_refs: Option<Box<IpReadResolution>>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Wire shadows discard unknown values rather than keeping IP payloads.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IpReferenceWire {
    #[serde(default)]
    pub ip_refs: Vec<EdgionConfigDataRef>,
    #[serde(default)]
    pub invalid_input: bool,
    #[serde(default)]
    pub resolved_refs: Option<Box<IpReadResolution>>,
    #[serde(flatten)]
    pub rejected: HashMap<String, IgnoredAny>,
}

impl IpReferenceWire {
    pub(crate) fn into_set(self) -> IpReferenceSet {
        IpReferenceSet {
            ip_refs: self.ip_refs,
            invalid_input: self.invalid_input || !self.rejected.is_empty(),
            resolved_refs: self.resolved_refs,
        }
    }
}

impl<'de> Deserialize<'de> for IpReferenceSet {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        IpReferenceWire::deserialize(deserializer).map(IpReferenceWire::into_set)
    }
}

impl IpReferenceSet {
    pub fn validate(&self) -> Result<(), String> {
        if self.invalid_input {
            return Err("Inline IP collections and unknown IP fields are not supported; use ipRefs".into());
        }
        if self.ip_refs.is_empty() {
            return Err("ipRefs must be configured".into());
        }
        for (index, reference) in self.ip_refs.iter().enumerate() {
            if reference.name.trim().is_empty() {
                return Err(format!("ipRefs[{index}].name cannot be empty"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_plugins/ip_reference_set_test.rs"]
mod tests;
