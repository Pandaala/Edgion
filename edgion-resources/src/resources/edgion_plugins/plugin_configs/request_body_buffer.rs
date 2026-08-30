//! RequestBodyBuffer plugin configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::edgion_plugins::PluginValidation;

/// Explicitly buffers the proxied request body for later plugins.
///
/// The body policy is configured on the containing request-plugin entry's
/// `body` field. This config is intentionally empty.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct RequestBodyBufferConfig {}

impl PluginValidation for RequestBodyBufferConfig {
    fn validation_error(&self) -> Option<String> {
        None
    }
}
