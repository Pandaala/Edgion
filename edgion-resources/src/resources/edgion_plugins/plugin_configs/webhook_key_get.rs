//! WebhookKeyGet plugin configuration
//!
//! Resolves webhook vars into request context once per request. Downstream
//! plugins read the written values through `KeyGet::Ctx`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::edgion_plugins::PluginValidation;

/// How to extract a key value from a webhook HTTP response.
///
/// ## YAML Examples
///
/// ```yaml
/// # From response header
/// extract:
///   source: header
///   name: "X-Resolved-Key"
///
/// # From response body (JSON path)
/// extract:
///   source: body
///   path: "data.user_id"
///
/// # From response cookie
/// extract:
///   source: cookie
///   name: "session_token"
///
/// # Entire response body as plain text
/// extract:
///   source: bodyText
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "source", rename_all = "camelCase")]
pub enum WebhookExtract {
    /// Extract from a response header
    Header {
        /// Response header name (case-insensitive)
        name: String,
    },

    /// Extract from a response cookie (Set-Cookie header)
    Cookie {
        /// Cookie name
        name: String,
    },

    /// Extract from response body using a dot-path into JSON
    ///
    /// Path format: "data.user_id", "result.0.key" (array index)
    /// The extracted value is converted to string.
    Body {
        /// Dot-path expression (e.g., "data.user_id")
        path: String,
    },

    /// Use entire response body as the key value (trimmed)
    BodyText,
}

/// WebhookKeyGet plugin configuration.
///
/// Resolves each var in configuration order and writes the result into the
/// request context.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct WebhookKeyGetConfig {
    /// List of webhook variables to resolve
    pub vars: Vec<WebhookVar>,
}

/// One webhook variable to resolve into request context.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebhookVar {
    /// Target context variable name
    pub name: String,

    /// Reference to a LinkSys Webhook resource: "namespace/name"
    pub webhook_ref: String,

    /// Optional path override for the webhook call.
    /// Only takes effect if the referenced webhook resource allows path overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_override: Option<String>,

    /// How to extract the value from the webhook response
    pub extract: WebhookExtract,

    /// Value written when resolution yields nothing
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

fn invalid_webhook_ref(webhook_ref: &str) -> Option<String> {
    let ref_parts: Vec<&str> = webhook_ref.split('/').collect();
    if ref_parts.len() != 2
        || ref_parts[0].is_empty()
        || ref_parts[1].is_empty()
        || !ref_parts
            .iter()
            .all(|p| p.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_'))
    {
        Some(format!(
            "Invalid webhookRef '{}': must be 'namespace/name' (alphanumeric, '-', '_' only)",
            webhook_ref
        ))
    } else {
        None
    }
}

fn extract_field_error(extract: &WebhookExtract) -> Option<String> {
    match extract {
        WebhookExtract::Header { name } if name.is_empty() => Some("extract header name is empty".to_string()),
        WebhookExtract::Cookie { name } if name.is_empty() => Some("extract cookie name is empty".to_string()),
        WebhookExtract::Body { path } if path.is_empty() => Some("extract body path is empty".to_string()),
        WebhookExtract::Header { .. }
        | WebhookExtract::Cookie { .. }
        | WebhookExtract::Body { .. }
        | WebhookExtract::BodyText => None,
    }
}

impl WebhookKeyGetConfig {
    /// Stateless field validation — mirrors every error branch of
    /// `validate_and_compile` without mutating self.
    pub fn check_fields(&self) -> Option<String> {
        if self.vars.is_empty() {
            return Some("vars list is empty".to_string());
        }
        for (idx, var) in self.vars.iter().enumerate() {
            if var.name.is_empty() {
                return Some(format!("vars[{}]: name is empty", idx));
            }
            if let Some(err) = invalid_webhook_ref(&var.webhook_ref) {
                return Some(format!("vars[{}] '{}': {}", idx, var.name, err));
            }
            if let Some(err) = extract_field_error(&var.extract) {
                return Some(format!("vars[{}] '{}': {}", idx, var.name, err));
            }
        }
        None
    }

    /// Validate configuration. Nothing is compiled; this is the mutating
    /// counterpart of [`check_fields`].
    pub fn validate_and_compile(&mut self) -> Result<(), String> {
        if let Some(err) = self.check_fields() {
            return Err(err);
        }
        Ok(())
    }

    /// Validate configuration (alias of [`validate_and_compile`]).
    pub fn validate(&mut self) -> Result<(), String> {
        self.validate_and_compile()
    }
}

impl PluginValidation for WebhookKeyGetConfig {
    fn validation_error(&self) -> Option<String> {
        self.check_fields()
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/webhook_key_get_test.rs"]
mod tests;
