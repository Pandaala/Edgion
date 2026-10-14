//! Physical-peer L4 ACL policy with reference-only IpList collections.
use crate::resources::edgion_plugins::ip_acl_refs::IpAclReferenceSet;
use crate::resources::edgion_plugins::{DefaultAction, PluginValidation};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionIpRestrictionRules {
    #[serde(flatten)]
    pub refs: IpAclReferenceSet,
    #[serde(default)]
    pub default_action: DefaultAction,
}
impl PluginValidation for ConnectionIpRestrictionRules {
    fn validation_error(&self) -> Option<String> {
        self.refs.validation_error()
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_stream_plugins/connection_ip_restriction_rules_test.rs"]
mod tests;
