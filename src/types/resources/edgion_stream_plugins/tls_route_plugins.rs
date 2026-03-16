//! TLS route stage plugin types for EdgionStreamPlugins.
//!
//! These plugins run at Stage 2 (post-TLS-handshake, post-route-match)
//! and have access to SNI, matched route info, mTLS status, etc.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::resources::edgion_plugins::IpRestrictionConfig;

/// Plugin enum for the TLS route stage.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "config")]
pub enum TlsRouteStreamPlugin {
    /// IP Restriction at TLS route level (same check, richer context)
    IpRestriction(IpRestrictionConfig),
}

impl TlsRouteStreamPlugin {
    pub fn type_name(&self) -> &'static str {
        match self {
            TlsRouteStreamPlugin::IpRestriction(_) => "IpRestriction",
        }
    }
}
