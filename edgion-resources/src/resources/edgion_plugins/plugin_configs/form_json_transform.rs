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
mod tests {
    use super::*;

    #[test]
    fn direction_is_required_and_uses_camel_case() {
        let form_to_json: FormJsonTransformConfig = serde_json::from_str(r#"{"direction":"formToJson"}"#).unwrap();
        assert_eq!(form_to_json.direction, FormJsonDirection::FormToJson);

        let json_to_form: FormJsonTransformConfig = serde_json::from_str(r#"{"direction":"jsonToForm"}"#).unwrap();
        assert_eq!(json_to_form.direction, FormJsonDirection::JsonToForm);

        assert!(serde_json::from_str::<FormJsonTransformConfig>("{}").is_err());
        assert!(serde_json::from_str::<FormJsonTransformConfig>(r#"{"direction":"auto"}"#).is_err());
    }
}
