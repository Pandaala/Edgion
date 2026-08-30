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
mod tests {
    use super::*;

    fn valid_var() -> WebhookVar {
        WebhookVar {
            name: "tenant_tier".to_string(),
            webhook_ref: "edgion-default/tenant-resolver".to_string(),
            path_override: None,
            extract: WebhookExtract::BodyText,
            default: None,
        }
    }

    fn valid_config() -> WebhookKeyGetConfig {
        WebhookKeyGetConfig {
            vars: vec![valid_var()],
        }
    }

    fn assert_check_matches_compile(config: &WebhookKeyGetConfig) {
        let check = config.check_fields();
        let mut compiled = config.clone();
        let compile_err = compiled.validate_and_compile().err();
        let mut validated = config.clone();
        let validate_err = validated.validate().err();
        assert_eq!(check, compile_err, "check_fields vs validate_and_compile");
        assert_eq!(check, validate_err, "check_fields vs validate");
        assert_eq!(config.validation_error(), check);
    }

    #[test]
    fn valid_config_succeeds() {
        let config = valid_config();
        assert!(config.check_fields().is_none());
        assert_check_matches_compile(&config);
        let mut compiled = config.clone();
        assert!(compiled.validate_and_compile().is_ok());
    }

    #[test]
    fn rejects_empty_vars() {
        let config = WebhookKeyGetConfig { vars: vec![] };
        assert_eq!(config.check_fields().as_deref(), Some("vars list is empty"));
        assert_check_matches_compile(&config);
    }

    #[test]
    fn rejects_empty_name() {
        let mut config = valid_config();
        config.vars[0].name.clear();
        assert_eq!(config.check_fields().as_deref(), Some("vars[0]: name is empty"));
        assert_check_matches_compile(&config);
    }

    #[test]
    fn rejects_webhook_ref_not_namespace_name() {
        let mut config = valid_config();
        config.vars[0].webhook_ref = "tenant-resolver".to_string();
        let err = config.check_fields().expect("must reject");
        assert!(err.contains("webhookRef"), "got: {err}");
        assert!(err.contains("namespace/name"), "got: {err}");
        assert_check_matches_compile(&config);
    }

    #[test]
    fn rejects_webhook_ref_empty_parts() {
        let mut config = valid_config();
        config.vars[0].webhook_ref = "edgion-default/".to_string();
        let err = config.check_fields().expect("must reject");
        assert!(err.contains("webhookRef"), "got: {err}");
        assert_check_matches_compile(&config);
    }

    #[test]
    fn rejects_webhook_ref_special_chars() {
        let mut config = valid_config();
        config.vars[0].webhook_ref = "default/webhook@svc".to_string();
        let err = config.check_fields().expect("must reject");
        assert!(err.contains("webhookRef"), "got: {err}");
        assert_check_matches_compile(&config);
    }

    #[test]
    fn rejects_empty_extract_header_name() {
        let mut config = valid_config();
        config.vars[0].extract = WebhookExtract::Header { name: String::new() };
        let err = config.check_fields().expect("must reject");
        assert!(err.contains("extract header name is empty"), "got: {err}");
        assert_check_matches_compile(&config);
    }

    #[test]
    fn rejects_empty_extract_cookie_name() {
        let mut config = valid_config();
        config.vars[0].extract = WebhookExtract::Cookie { name: String::new() };
        let err = config.check_fields().expect("must reject");
        assert!(err.contains("extract cookie name is empty"), "got: {err}");
        assert_check_matches_compile(&config);
    }

    #[test]
    fn rejects_empty_extract_body_path() {
        let mut config = valid_config();
        config.vars[0].extract = WebhookExtract::Body { path: String::new() };
        let err = config.check_fields().expect("must reject");
        assert!(err.contains("extract body path is empty"), "got: {err}");
        assert_check_matches_compile(&config);
    }

    #[test]
    fn body_text_extract_is_always_well_formed() {
        let mut config = valid_config();
        config.vars[0].extract = WebhookExtract::BodyText;
        assert!(config.check_fields().is_none());
        assert_check_matches_compile(&config);
    }

    #[test]
    fn yaml_roundtrip_camel_case() {
        let yaml = r#"
vars:
  - name: tenant_tier
    webhookRef: "edgion-default/tenant-resolver"
    pathOverride: "/resolve/tier"
    extract:
      source: body
      path: "data.tier"
    default: "free"
"#;
        let config: WebhookKeyGetConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.vars.len(), 1);
        assert_eq!(config.vars[0].name, "tenant_tier");
        assert_eq!(config.vars[0].webhook_ref, "edgion-default/tenant-resolver");
        assert_eq!(config.vars[0].path_override.as_deref(), Some("/resolve/tier"));
        assert_eq!(
            config.vars[0].extract,
            WebhookExtract::Body {
                path: "data.tier".to_string()
            }
        );
        assert_eq!(config.vars[0].default.as_deref(), Some("free"));
        assert!(config.check_fields().is_none());
    }

    #[test]
    fn test_webhook_extract_serde() {
        let yaml = r#"source: header
name: "X-Key""#;
        let extract: WebhookExtract = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            extract,
            WebhookExtract::Header {
                name: "X-Key".to_string()
            }
        );

        let yaml = r#"source: body
path: "result.value""#;
        let extract: WebhookExtract = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            extract,
            WebhookExtract::Body {
                path: "result.value".to_string()
            }
        );

        let yaml = "source: bodyText";
        let extract: WebhookExtract = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(extract, WebhookExtract::BodyText);

        let yaml = r#"source: cookie
name: "sess""#;
        let extract: WebhookExtract = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            extract,
            WebhookExtract::Cookie {
                name: "sess".to_string()
            }
        );
    }
}
