//! Stream plugin types for EdgionStreamPlugins

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::resources::edgion_plugins::IpRestrictionConfig;

/// Stream plugin enum for all supported stream plugin types
///
/// Currently supports:
/// - IpRestriction: IP-based access control for TCP/UDP connections
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "config", rename_all = "camelCase")]
pub enum EdgionStreamPlugin {
    /// IP Restriction filter (allow/deny based on IP address or CIDR)
    /// Controls access to TCP/UDP connections based on client IP
    IpRestriction(IpRestrictionConfig),
    
    // TODO: Add more stream plugins in the future
    // RateLimit(StreamRateLimitConfig),
    // ConnectionLimit(ConnectionLimitConfig),
}

impl EdgionStreamPlugin {
    /// Get the type name of this plugin
    pub fn type_name(&self) -> &'static str {
        match self {
            EdgionStreamPlugin::IpRestriction(_) => "IpRestriction",
        }
    }
}

