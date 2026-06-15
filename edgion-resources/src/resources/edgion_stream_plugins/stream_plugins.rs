//! Stream plugin types for EdgionStreamPlugins

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::data::DataRef;
use crate::resources::edgion_stream_plugins::connection_ip_restriction_rules::ConnectionIpRestrictionRules;
use crate::resources::edgion_stream_plugins::connection_rate_limit_config::ConnectionRateLimitConfig;

/// Stream plugin enum for all supported stream plugin types.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "config")]
pub enum EdgionStreamPlugin {
    /// Inline IP restriction (L4-specific, no HTTP fields).
    IpRestriction(ConnectionIpRestrictionRules),

    /// PM-referenced IP restriction; Center-managed, multi-Controller synced.
    GlobalConnectionIpRestriction(GlobalConnectionIpRestrictionEntry),

    /// L4 connection-rate limiter (local gate + optional Redis).
    ConnectionRateLimit(ConnectionRateLimitConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GlobalConnectionIpRestrictionEntry {
    /// Reference to a GlobalConnectionIpRestriction EdgionData.
    /// Namespace is inherited from parent EdgionStreamPlugins if omitted.
    pub plugin_metadata_ref: DataRef,
}

impl EdgionStreamPlugin {
    pub fn type_name(&self) -> &'static str {
        match self {
            EdgionStreamPlugin::IpRestriction(_) => "IpRestriction",
            EdgionStreamPlugin::GlobalConnectionIpRestriction(_) => "GlobalConnectionIpRestriction",
            EdgionStreamPlugin::ConnectionRateLimit(_) => "ConnectionRateLimit",
        }
    }
}

#[cfg(test)]
mod variant_tests {
    use super::*;

    #[test]
    fn connection_rate_limit_deserializes_with_tag_content_shape() {
        let yaml = r#"
type: ConnectionRateLimit
config:
  redisRef: "default/redis"
  perSourceIp:
    rate: 50
    interval: "1s"
"#;
        let p: EdgionStreamPlugin = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(p.type_name(), "ConnectionRateLimit");
    }
}
