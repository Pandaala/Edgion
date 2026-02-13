//! Key Auth plugin configuration
//!
//! API Key authentication plugin that validates requests against configured keys
//! stored in Kubernetes Secrets.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::types::common::KeyGet;
use crate::types::resources::gateway::SecretObjectReference;

/// Key Auth plugin configuration
///
/// Supports API key authentication from various sources (header, query, cookie, etc.).
/// Keys are stored in Kubernetes Secrets in YAML format.
///
/// ## Key Source Configuration
///
/// Use `key_sources` to specify where to look for the API key. Sources are tried in order.
///
/// ```yaml
/// keySources:
///   - type: header
///     name: "X-API-Key"
///   - type: query
///     name: "api_key"
///   - type: cookie
///     name: "api_key"
/// ```
///
/// If not specified, defaults to header "apikey" then query "apikey".
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeyAuthConfig {
    // === Key Source ===
    /// Sources to extract API key from (tried in order)
    /// Supports: header, query, cookie, ctx
    /// Default: [header:"apikey", query:"apikey"]
    #[serde(default = "default_key_sources")]
    pub key_sources: Vec<KeyGet>,

    // === Security Options ===
    /// Remove API key from request before forwarding to upstream (default: false)
    #[serde(default)]
    pub hide_credentials: bool,

    /// Username for anonymous access when no key provided (optional)
    /// If set, requests without key will be allowed with this username
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anonymous: Option<String>,

    /// Realm for WWW-Authenticate header (default: "API Gateway")
    #[serde(default = "default_realm")]
    pub realm: String,

    // === Key Storage ===
    /// Secret field name that contains the API key value (default: "key")
    #[serde(default = "default_key_field")]
    pub key_field: String,

    /// Secret references containing API keys
    /// Each Secret should contain a 'keys.yaml' field with key entries
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_refs: Option<Vec<SecretObjectReference>>,

    // === Upstream Headers ===
    /// Whitelist: Secret fields to add as upstream headers
    /// Only fields listed here will be forwarded to upstream
    /// If empty, no additional headers will be added
    #[serde(default)]
    pub upstream_header_fields: Vec<String>,

    // === Runtime fields (populated by controller) ===
    /// Resolved API Keys mapping: key -> metadata
    /// This is populated during Secret parsing and should not be set by users
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_keys: Option<HashMap<String, KeyMetadata>>,

    // === Validation fields ===
    /// Validation error message (populated during preparse)
    #[serde(skip)]
    #[schemars(skip)]
    pub validation_error: Option<String>,
}

/// API Key metadata containing headers to forward to upstream
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KeyMetadata {
    /// Headers to forward to upstream (field_name -> value)
    pub headers: HashMap<String, String>,
}

// === Default value functions ===

fn default_key_sources() -> Vec<KeyGet> {
    vec![
        KeyGet::Header {
            name: "apikey".to_string(),
        },
        KeyGet::Query {
            name: "apikey".to_string(),
        },
    ]
}

fn default_realm() -> String {
    "API Gateway".to_string()
}

fn default_key_field() -> String {
    "key".to_string()
}

impl Default for KeyAuthConfig {
    fn default() -> Self {
        Self {
            key_sources: default_key_sources(),
            hide_credentials: false,
            anonymous: None,
            realm: default_realm(),
            key_field: default_key_field(),
            secret_refs: None,
            upstream_header_fields: vec![],
            resolved_keys: None,
            validation_error: None,
        }
    }
}

impl KeyAuthConfig {
    /// Validate configuration and return true if valid
    pub fn validate(&mut self) -> bool {
        match self.validate_and_check() {
            Ok(()) => {
                self.validation_error = None;
                true
            }
            Err(e) => {
                self.validation_error = Some(e);
                false
            }
        }
    }

    /// Check if configuration is valid
    pub fn is_valid(&self) -> bool {
        self.validation_error.is_none()
    }

    /// Get validation error message
    pub fn get_validation_error(&self) -> Option<&str> {
        self.validation_error.as_deref()
    }

    /// Internal validation logic
    fn validate_and_check(&self) -> Result<(), String> {
        // Check that key_sources is not empty
        if self.key_sources.is_empty() {
            return Err("key_sources cannot be empty - at least one key source must be specified".to_string());
        }

        // Validate each key source
        for (i, source) in self.key_sources.iter().enumerate() {
            match source {
                KeyGet::Header { name } | KeyGet::Query { name } | KeyGet::Cookie { name } | KeyGet::Ctx { name } => {
                    if name.is_empty() {
                        return Err(format!("key_sources[{}]: name cannot be empty", i));
                    }
                }
                // Webhook is a valid key source for remote key resolution
                KeyGet::Webhook { webhook_ref, .. } => {
                    if webhook_ref.is_empty() {
                        return Err(format!("key_sources[{}]: webhookRef cannot be empty", i));
                    }
                }
                // Unsupported sources for API key extraction
                KeyGet::ClientIp | KeyGet::Path | KeyGet::Method | KeyGet::ClientIpAndPath => {
                    return Err(format!(
                        "key_sources[{}]: unsupported source type '{}' for API key extraction. Use header, query, cookie, ctx, or webhook",
                        i,
                        source.source_type()
                    ));
                }
            }
        }

        // Validate key_field is not empty
        if self.key_field.is_empty() {
            return Err("key_field cannot be empty".to_string());
        }

        // Validate realm is not empty (optional, but warn if empty)
        if self.realm.is_empty() {
            return Err("realm cannot be empty".to_string());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = KeyAuthConfig::default();
        assert_eq!(config.key_sources.len(), 2);
        assert_eq!(
            config.key_sources[0],
            KeyGet::Header {
                name: "apikey".to_string()
            }
        );
        assert_eq!(
            config.key_sources[1],
            KeyGet::Query {
                name: "apikey".to_string()
            }
        );
        assert_eq!(config.realm, "API Gateway");
        assert_eq!(config.key_field, "key");
        assert!(!config.hide_credentials);
        assert!(config.anonymous.is_none());
        assert!(config.secret_refs.is_none());
        assert!(config.upstream_header_fields.is_empty());
        assert!(config.resolved_keys.is_none());
    }

    #[test]
    fn test_deserialize_minimal_config() {
        let json = r#"{}"#;
        let config: KeyAuthConfig = serde_json::from_str(json).unwrap();
        // Should use defaults
        assert_eq!(config.key_sources.len(), 2);
    }

    #[test]
    fn test_deserialize_full_config() {
        let json = r#"{
            "keySources": [
                {"type": "header", "name": "X-API-Key"},
                {"type": "query", "name": "api_key"},
                {"type": "cookie", "name": "api_token"}
            ],
            "hideCredentials": true,
            "anonymous": "guest",
            "realm": "My API",
            "keyField": "apiKey",
            "upstreamHeaderFields": ["X-Consumer-Username", "X-Customer-ID"]
        }"#;
        let config: KeyAuthConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.key_sources.len(), 3);
        assert_eq!(
            config.key_sources[0],
            KeyGet::Header {
                name: "X-API-Key".to_string()
            }
        );
        assert_eq!(
            config.key_sources[1],
            KeyGet::Query {
                name: "api_key".to_string()
            }
        );
        assert_eq!(
            config.key_sources[2],
            KeyGet::Cookie {
                name: "api_token".to_string()
            }
        );
        assert!(config.hide_credentials);
        assert_eq!(config.anonymous, Some("guest".to_string()));
        assert_eq!(config.realm, "My API");
        assert_eq!(config.key_field, "apiKey");
        assert_eq!(config.upstream_header_fields.len(), 2);
    }

    #[test]
    fn test_deserialize_yaml_config() {
        let yaml = r#"
keySources:
  - type: header
    name: X-API-Key
  - type: query
    name: api_key
hideCredentials: true
realm: "Production API"
"#;
        let config: KeyAuthConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.key_sources.len(), 2);
        assert!(config.hide_credentials);
        assert_eq!(config.realm, "Production API");
    }

    #[test]
    fn test_key_metadata_default() {
        let metadata = KeyMetadata::default();
        assert!(metadata.headers.is_empty());
    }

    // Validation tests

    #[test]
    fn test_validate_default_config() {
        let mut config = KeyAuthConfig::default();
        assert!(config.validate());
        assert!(config.is_valid());
        assert!(config.get_validation_error().is_none());
    }

    #[test]
    fn test_validate_empty_key_sources() {
        let mut config = KeyAuthConfig {
            key_sources: vec![],
            ..Default::default()
        };
        assert!(!config.validate());
        assert!(!config.is_valid());
        assert_eq!(
            config.get_validation_error(),
            Some("key_sources cannot be empty - at least one key source must be specified")
        );
    }

    #[test]
    fn test_validate_empty_name_in_key_source() {
        let mut config = KeyAuthConfig {
            key_sources: vec![KeyGet::Header { name: "".to_string() }],
            ..Default::default()
        };
        assert!(!config.validate());
        assert_eq!(
            config.get_validation_error(),
            Some("key_sources[0]: name cannot be empty")
        );
    }

    #[test]
    fn test_validate_unsupported_key_source() {
        let mut config = KeyAuthConfig {
            key_sources: vec![KeyGet::ClientIp],
            ..Default::default()
        };
        assert!(!config.validate());
        let error = config.get_validation_error().unwrap();
        assert!(error.contains("unsupported source type"));
        assert!(error.contains("clientIp"));
    }

    #[test]
    fn test_validate_empty_key_field() {
        let mut config = KeyAuthConfig {
            key_field: "".to_string(),
            ..Default::default()
        };
        assert!(!config.validate());
        assert_eq!(config.get_validation_error(), Some("key_field cannot be empty"));
    }

    #[test]
    fn test_validate_empty_realm() {
        let mut config = KeyAuthConfig {
            realm: "".to_string(),
            ..Default::default()
        };
        assert!(!config.validate());
        assert_eq!(config.get_validation_error(), Some("realm cannot be empty"));
    }

    #[test]
    fn test_validate_valid_custom_config() {
        let mut config = KeyAuthConfig {
            key_sources: vec![
                KeyGet::Header {
                    name: "X-API-Key".to_string(),
                },
                KeyGet::Query {
                    name: "api_key".to_string(),
                },
                KeyGet::Cookie {
                    name: "token".to_string(),
                },
            ],
            key_field: "apiKey".to_string(),
            realm: "My API".to_string(),
            ..Default::default()
        };
        assert!(config.validate());
        assert!(config.is_valid());
    }
}
