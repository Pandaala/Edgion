//! Concurrent-connection cap config for `EdgionStreamPlugin::ConnectionLimit`.
//!
//! The cap applies to the attachment scope that runs the plugin: a Gateway
//! annotation caps live sockets per physical listener at accept (before PROXY
//! parsing and the TLS handshake); a TCPRoute annotation caps connections in
//! that TCP listener's route stage; a UDPRoute annotation caps that UDP
//! listener's sessions. The count is owned by the listener, not by the plugin.

use crate::resources::edgion_plugins::PluginValidation;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Largest accepted `maxConnections`.
pub const MAX_CONNECTION_LIMIT: u64 = 10_000_000;

/// Concurrent-connection cap for one attachment scope.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionLimitConfig {
    /// Maximum concurrent connections (UDP: sessions) admitted in the
    /// attachment scope. A connection beyond the cap is closed.
    #[schemars(range(min = 1, max = 10000000))]
    pub max_connections: u64,
}

impl PluginValidation for ConnectionLimitConfig {
    fn validation_error(&self) -> Option<String> {
        if !(1..=MAX_CONNECTION_LIMIT).contains(&self.max_connections) {
            return Some(format!(
                "'maxConnections' must be between 1 and {MAX_CONNECTION_LIMIT}, got {}",
                self.max_connections
            ));
        }
        None
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_stream_plugins/connection_limit_config_test.rs"]
mod tests;
