//! DebugAccessLogToHeader plugin configuration

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// DebugAccessLogToHeader plugin configuration
/// 
/// This plugin adds the current access log as a JSON string to the response header
/// for debugging purposes. No configuration is required - simply enable the plugin.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DebugAccessLogToHeaderConfig {}

impl Default for DebugAccessLogToHeaderConfig {
    fn default() -> Self {
        Self {}
    }
}

