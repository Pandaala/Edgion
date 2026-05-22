//! Response Rewrite plugin configuration
//!
//! Rewrites responses before returning to client.
//! Supports status code and headers modification.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Response Rewrite plugin configuration
///
/// Rewrites responses before returning to client.
/// All fields are optional - configure only what you need.
///
/// # Example
///
/// ```yaml
/// type: ResponseRewrite
/// config:
///   statusCode: 200
///   headers:
///     set:
///       - name: Cache-Control
///         value: "no-cache"
///     add:
///       - name: X-Powered-By
///         value: "Edgion"
///     remove:
///       - Server
///     rename:
///       - from: X-Internal-Id
///         to: X-Request-Id
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct ResponseRewriteConfig {
    /// New HTTP status code (100-599).
    ///
    /// Overrides the upstream response status code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,

    /// Response header operations.
    ///
    /// Execution order: rename -> add -> set -> remove.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<ResponseHeaderActions>,
}

/// Response header operations configuration.
///
/// Uses Vec instead of HashMap to preserve insertion order.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResponseHeaderActions {
    /// Set response headers (overwrite existing values).
    /// Headers are processed in order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub set: Option<Vec<ResponseHeaderEntry>>,

    /// Add response headers (append to existing values).
    /// Headers are processed in order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub add: Option<Vec<ResponseHeaderEntry>>,

    /// Remove response headers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remove: Option<Vec<String>>,

    /// Rename response headers.
    /// Headers are processed in order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rename: Option<Vec<HeaderRename>>,
}

/// A single header name-value pair for response headers.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResponseHeaderEntry {
    /// Header name
    pub name: String,
    /// Header value
    pub value: String,
}

/// Header rename configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HeaderRename {
    /// Original header name to rename from
    pub from: String,
    /// New header name to rename to
    pub to: String,
}

impl ResponseRewriteConfig {
    /// Stateless field validation.
    ///
    /// Returns `Some(error)` if any field is invalid, `None` if all checks pass.
    /// Does not mutate `self`.
    pub fn check_fields(&self) -> Option<String> {
        // Validate status code range
        if let Some(status) = self.status_code {
            if !(100..=599).contains(&status) {
                return Some(format!("Invalid status code: {}. Must be between 100 and 599.", status));
            }
        }

        // Validate header names are not empty
        if let Some(ref headers) = self.headers {
            // Check set headers
            if let Some(ref set_headers) = headers.set {
                for entry in set_headers {
                    if entry.name.is_empty() {
                        return Some("Header name in 'set' cannot be empty.".to_string());
                    }
                }
            }

            // Check add headers
            if let Some(ref add_headers) = headers.add {
                for entry in add_headers {
                    if entry.name.is_empty() {
                        return Some("Header name in 'add' cannot be empty.".to_string());
                    }
                }
            }

            // Check remove headers
            if let Some(ref remove_headers) = headers.remove {
                for name in remove_headers {
                    if name.is_empty() {
                        return Some("Header name in 'remove' cannot be empty.".to_string());
                    }
                }
            }

            // Check rename headers
            if let Some(ref rename_headers) = headers.rename {
                for entry in rename_headers {
                    if entry.from.is_empty() {
                        return Some("Header 'from' name in 'rename' cannot be empty.".to_string());
                    }
                    if entry.to.is_empty() {
                        return Some("Header 'to' name in 'rename' cannot be empty.".to_string());
                    }
                }
            }
        }

        None
    }

    /// Check if any rewrite is configured (for logging purposes)
    pub fn has_any_config(&self) -> bool {
        self.status_code.is_some() || self.headers.is_some()
    }
}

use crate::types::resources::edgion_plugins::PluginValidation;

impl PluginValidation for ResponseRewriteConfig {
    fn validation_error(&self) -> Option<String> {
        self.check_fields()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_response_rewrite_config_default() {
        let config = ResponseRewriteConfig::default();
        assert!(config.status_code.is_none());
        assert!(config.headers.is_none());
        assert!(config.check_fields().is_none());
        assert!(!config.has_any_config());
    }

    #[test]
    fn test_response_rewrite_config_validate_valid() {
        let config = ResponseRewriteConfig {
            status_code: Some(200),
            headers: Some(ResponseHeaderActions {
                set: Some(vec![ResponseHeaderEntry {
                    name: "X-Custom".to_string(),
                    value: "value".to_string(),
                }]),
                add: None,
                remove: None,
                rename: None,
            }),
        };

        assert!(config.check_fields().is_none());
    }

    #[test]
    fn test_response_rewrite_config_validate_invalid_status() {
        let config = ResponseRewriteConfig {
            status_code: Some(600), // Invalid
            headers: None,
        };

        assert!(config.check_fields().unwrap().contains("Invalid status code"));
    }

    #[test]
    fn test_response_rewrite_config_validate_status_range() {
        // Test lower bound
        let config = ResponseRewriteConfig {
            status_code: Some(100),
            ..Default::default()
        };
        assert!(config.check_fields().is_none());

        // Test upper bound
        let config = ResponseRewriteConfig {
            status_code: Some(599),
            ..Default::default()
        };
        assert!(config.check_fields().is_none());

        // Test below lower bound
        let config = ResponseRewriteConfig {
            status_code: Some(99),
            ..Default::default()
        };
        assert!(config.check_fields().is_some());

        // Test above upper bound
        let config = ResponseRewriteConfig {
            status_code: Some(600),
            ..Default::default()
        };
        assert!(config.check_fields().is_some());
    }

    #[test]
    fn test_response_rewrite_config_validate_empty_header_name() {
        let config = ResponseRewriteConfig {
            status_code: None,
            headers: Some(ResponseHeaderActions {
                set: Some(vec![ResponseHeaderEntry {
                    name: "".to_string(), // Empty name
                    value: "value".to_string(),
                }]),
                add: None,
                remove: None,
                rename: None,
            }),
        };

        assert!(config.check_fields().unwrap().contains("cannot be empty"));
    }

    #[test]
    fn test_response_rewrite_config_validate_empty_rename() {
        // Empty 'from'
        let config = ResponseRewriteConfig {
            status_code: None,
            headers: Some(ResponseHeaderActions {
                set: None,
                add: None,
                remove: None,
                rename: Some(vec![HeaderRename {
                    from: "".to_string(),
                    to: "X-New".to_string(),
                }]),
            }),
        };

        assert!(config.check_fields().unwrap().contains("'from'"));

        // Empty 'to'
        let config = ResponseRewriteConfig {
            status_code: None,
            headers: Some(ResponseHeaderActions {
                set: None,
                add: None,
                remove: None,
                rename: Some(vec![HeaderRename {
                    from: "X-Old".to_string(),
                    to: "".to_string(),
                }]),
            }),
        };

        assert!(config.check_fields().unwrap().contains("'to'"));
    }

    #[test]
    fn test_response_rewrite_config_serde() {
        let json = r#"{
            "statusCode": 201,
            "headers": {
                "set": [
                    {"name": "X-Custom", "value": "custom-value"}
                ],
                "add": [
                    {"name": "X-Added", "value": "added-value"}
                ],
                "remove": ["Server", "X-Debug"],
                "rename": [
                    {"from": "X-Old", "to": "X-New"}
                ]
            }
        }"#;

        let config: ResponseRewriteConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.status_code, Some(201));

        let headers = config.headers.as_ref().unwrap();

        let set_headers = headers.set.as_ref().unwrap();
        assert_eq!(set_headers.len(), 1);
        assert_eq!(set_headers[0].name, "X-Custom");
        assert_eq!(set_headers[0].value, "custom-value");

        let add_headers = headers.add.as_ref().unwrap();
        assert_eq!(add_headers.len(), 1);
        assert_eq!(add_headers[0].name, "X-Added");

        let remove_headers = headers.remove.as_ref().unwrap();
        assert_eq!(remove_headers.len(), 2);
        assert!(remove_headers.contains(&"Server".to_string()));
        assert!(remove_headers.contains(&"X-Debug".to_string()));

        let rename_headers = headers.rename.as_ref().unwrap();
        assert_eq!(rename_headers.len(), 1);
        assert_eq!(rename_headers[0].from, "X-Old");
        assert_eq!(rename_headers[0].to, "X-New");
    }

    #[test]
    fn test_header_order_preserved() {
        let json = r#"{
            "headers": {
                "set": [
                    {"name": "X-First", "value": "1"},
                    {"name": "X-Second", "value": "2"},
                    {"name": "X-Third", "value": "3"}
                ]
            }
        }"#;

        let config: ResponseRewriteConfig = serde_json::from_str(json).unwrap();
        let set_headers = config.headers.as_ref().unwrap().set.as_ref().unwrap();
        assert_eq!(set_headers[0].name, "X-First");
        assert_eq!(set_headers[1].name, "X-Second");
        assert_eq!(set_headers[2].name, "X-Third");
    }
}

#[cfg(test)]
mod validation_trait_tests {
    use super::*;
    use crate::types::resources::edgion_plugins::PluginValidation;

    #[test]
    fn validation_error_returns_field_error_without_validate_call() {
        let config = ResponseRewriteConfig {
            status_code: Some(600), // out of range, triggers a field-level error
            headers: None,
        };

        let result = config.validation_error();
        assert!(
            result.as_deref().unwrap_or("").contains("Invalid status code: 600"),
            "expected invalid status code error, got: {:?}",
            result,
        );
    }

    #[test]
    fn validation_error_returns_none_for_valid_config() {
        let config = ResponseRewriteConfig {
            status_code: Some(200),
            headers: None,
        };
        assert!(config.validation_error().is_none());
    }

    #[test]
    fn validation_error_returns_none_for_default_config() {
        let config = ResponseRewriteConfig::default();
        assert!(config.validation_error().is_none());
    }

    #[test]
    fn validation_error_detects_empty_set_header_name() {
        let config = ResponseRewriteConfig {
            status_code: None,
            headers: Some(ResponseHeaderActions {
                set: Some(vec![ResponseHeaderEntry {
                    name: "".to_string(),
                    value: "v".to_string(),
                }]),
                add: None,
                remove: None,
                rename: None,
            }),
        };
        let result = config.validation_error();
        assert!(
            result.as_deref().unwrap_or("").contains("cannot be empty"),
            "expected empty-header error, got: {:?}",
            result,
        );
    }
}
