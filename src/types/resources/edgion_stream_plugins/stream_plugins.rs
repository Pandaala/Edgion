//! Stream plugin types for EdgionStreamPlugins

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::resources::edgion_gateway_config::PluginReference;
use crate::types::resources::edgion_stream_plugins::connection_ip_restriction_rules::ConnectionIpRestrictionRules;

/// Stream plugin enum for all supported stream plugin types.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "config")]
pub enum EdgionStreamPlugin {
    /// Inline IP restriction (L4-specific, no HTTP fields).
    IpRestriction(ConnectionIpRestrictionRules),

    /// PM-referenced IP restriction; Center-managed, multi-Controller synced.
    GlobalConnectionIpRestriction(GlobalConnectionIpRestrictionEntry),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GlobalConnectionIpRestrictionEntry {
    /// Reference to a GlobalConnectionIpRestriction PluginMetaData.
    /// Namespace is inherited from parent EdgionStreamPlugins if omitted.
    pub plugin_metadata_ref: PluginReference,
}

impl EdgionStreamPlugin {
    pub fn type_name(&self) -> &'static str {
        match self {
            EdgionStreamPlugin::IpRestriction(_) => "IpRestriction",
            EdgionStreamPlugin::GlobalConnectionIpRestriction(_) => "GlobalConnectionIpRestriction",
        }
    }
}
