//! HTTP/TLS-route ACL policy. IP collections live exclusively in referenced IpLists.
use crate::resources::edgion_plugins::ip_acl_refs::{IpAclReferenceSet, IpAclWire};
use crate::resources::edgion_plugins::PluginValidation;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IpRestrictionConfig {
    #[serde(flatten)]
    pub refs: IpAclReferenceSet,
    /// Resolved remote address or raw physical transport peer.
    #[serde(default)]
    pub ip_source: IpSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default = "default_status")]
    #[schemars(range(min = 200, max = 599))]
    pub status: u16,
    #[serde(default)]
    pub default_action: DefaultAction,
}
impl<'de> Deserialize<'de> for IpRestrictionConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            #[serde(default)]
            ip_source: IpSource,
            #[serde(default)]
            message: Option<String>,
            #[serde(default = "default_status")]
            status: u16,
            #[serde(default)]
            default_action: DefaultAction,
            #[serde(flatten)]
            refs: IpAclWire,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            refs: wire.refs.into_set(),
            ip_source: wire.ip_source,
            message: wire.message,
            status: wire.status,
            default_action: wire.default_action,
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "PascalCase")]
pub enum IpSource {
    /// Canonical RealIp result, falling back to the effective transport peer.
    #[default]
    RemoteIp,
    /// Physical TCP peer, independent of forwarded headers and PROXY protocol.
    DirectPeerIp,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum DefaultAction {
    #[default]
    Allow,
    Deny,
}
fn default_status() -> u16 {
    403
}
impl Default for IpRestrictionConfig {
    fn default() -> Self {
        Self {
            refs: IpAclReferenceSet::default(),
            ip_source: IpSource::default(),
            message: None,
            status: default_status(),
            default_action: DefaultAction::default(),
        }
    }
}
impl PluginValidation for IpRestrictionConfig {
    fn validation_error(&self) -> Option<String> {
        self.refs.validation_error().or_else(|| {
            (!crate::http_status::is_valid_final_http_status(self.status))
                .then(|| format!("Invalid status code: {}", self.status))
        })
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/ip_restriction_test.rs"]
mod tests;
#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/ip_restriction_test/validation_trait_tests.rs"]
mod validation_trait_tests;
