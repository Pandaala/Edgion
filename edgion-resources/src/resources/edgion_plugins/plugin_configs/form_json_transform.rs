//! Request-body transformation between form URL encoding and flat JSON.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::edgion_plugins::PluginValidation;

/// Direction of the request-body transformation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum FormJsonDirection {
    FormToJson,
    JsonToForm,
}

/// Configuration for request-body form and JSON transformation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FormJsonTransformConfig {
    /// Explicit transformation direction.
    pub direction: FormJsonDirection,
}

impl PluginValidation for FormJsonTransformConfig {
    fn validation_error(&self) -> Option<String> {
        None
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/form_json_transform_test.rs"]
mod tests;
