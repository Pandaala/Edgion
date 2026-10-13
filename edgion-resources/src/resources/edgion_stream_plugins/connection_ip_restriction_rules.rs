//! Physical-peer L4 ACL policy with reference-only IpList collections.
use crate::resources::edgion_plugins::ip_acl_refs::{IpAclReferenceSet, IpAclWire};
use crate::resources::edgion_plugins::{DefaultAction, PluginValidation};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const LEGACY_HTTP_FIELDS: &[&str] = &["ipSource", "message", "status"];
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionIpRestrictionRules {
    #[serde(flatten)]
    pub refs: IpAclReferenceSet,
    #[serde(default)]
    pub default_action: DefaultAction,
    /// Bounded presence mask; original rejected field values are discarded.
    #[serde(default, skip_serializing_if = "is_zero")]
    #[schemars(skip)]
    legacy_http_fields: u8,
}
fn is_zero(value: &u8) -> bool {
    *value == 0
}
impl<'de> Deserialize<'de> for ConnectionIpRestrictionRules {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            #[serde(default)]
            default_action: DefaultAction,
            #[serde(default)]
            legacy_http_fields: u8,
            #[serde(flatten)]
            refs: IpAclWire,
        }
        let wire = Wire::deserialize(deserializer)?;
        let detected = LEGACY_HTTP_FIELDS.iter().enumerate().fold(0, |mask, (index, name)| {
            mask | if wire.refs.contains(name) { 1 << index } else { 0 }
        });
        Ok(Self {
            default_action: wire.default_action,
            legacy_http_fields: wire.legacy_http_fields | detected,
            refs: wire.refs.into_set(),
        })
    }
}
impl ConnectionIpRestrictionRules {
    pub fn legacy_http_fields(&self) -> Vec<&'static str> {
        LEGACY_HTTP_FIELDS
            .iter()
            .enumerate()
            .filter(|(index, _)| self.legacy_http_fields & (1 << index) != 0)
            .map(|(_, name)| *name)
            .collect()
    }
}
impl PluginValidation for ConnectionIpRestrictionRules {
    fn validation_error(&self) -> Option<String> {
        if self.legacy_http_fields != 0 {
            return Some("HTTP-only fields are not supported in connection IP restriction".into());
        }
        self.refs.validation_error()
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_stream_plugins/connection_ip_restriction_rules_test.rs"]
mod tests;
