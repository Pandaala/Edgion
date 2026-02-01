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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

    // ========== Runtime fields (not serialized) ==========
    /// Configuration validation error.
    #[serde(skip)]
    #[schemars(skip)]
    pub validation_error: Option<String>,
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

impl Default for ResponseRewriteConfig {
    fn default() -> Self {
        Self {
            status_code: None,
            headers: None,
            validation_error: None,
        }
    }
}

impl ResponseRewriteConfig {
    /// Validate configuration.
    ///
    /// Returns true if configuration is valid, false otherwise.
    pub fn validate(&mut self) -> bool {
        // Validate status code range
        if let Some(status) = self.status_code {
            if !(100..=599).contains(&status) {
                self.validation_error = Some(format!(
                    "Invalid status code: {}. Must be between 100 and 599.",
                    status
                ));
                return false;
            }
        }

        // Validate header names are not empty
        if let Some(ref headers) = self.headers {
            // Check set headers
            if let Some(ref set_headers) = headers.set {
                for entry in set_headers {
                    if entry.name.is_empty() {
                        self.validation_error = Some("Header name in 'set' cannot be empty.".to_string());
                        return false;
                    }
                }
            }

            // Check add headers
            if let Some(ref add_headers) = headers.add {
                for entry in add_headers {
                    if entry.name.is_empty() {
                        self.validation_error = Some("Header name in 'add' cannot be empty.".to_string());
                        return false;
                    }
                }
            }

            // Check remove headers
            if let Some(ref remove_headers) = headers.remove {
                for name in remove_headers {
                    if name.is_empty() {
                        self.validation_error = Some("Header name in 'remove' cannot be empty.".to_string());
                        return false;
                    }
                }
            }

            // Check rename headers
            if let Some(ref rename_headers) = headers.rename {
                for entry in rename_headers {
                    if entry.from.is_empty() {
                        self.validation_error =
                            Some("Header 'from' name in 'rename' cannot be empty.".to_string());
                        return false;
                    }
                    if entry.to.is_empty() {
                        self.validation_error = Some("Header 'to' name in 'rename' cannot be empty.".to_string());
                        return false;
                    }
                }
            }
        }

        self.validation_error = None;
        true
    }

    /// Check if configuration is valid.
    pub fn is_valid(&self) -> bool {
        self.validation_error.is_none()
    }

    /// Get validation error message.
    pub fn get_validation_error(&self) -> Option<&str> {
        self.validation_error.as_deref()
    }

    /// Check if any rewrite is configured (for logging purposes)
    pub fn has_any_config(&self) -> bool {
        self.status_code.is_some() || self.headers.is_some()
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
        assert!(config.is_valid());
        assert!(!config.has_any_config());
    }

    #[test]
    fn test_response_rewrite_config_validate_valid() {
        let mut config = ResponseRewriteConfig {
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
            validation_error: None,
        };

        assert!(config.validate());
        assert!(config.is_valid());
    }

    #[test]
    fn test_response_rewrite_config_validate_invalid_status() {
        let mut config = ResponseRewriteConfig {
            status_code: Some(600), // Invalid
            headers: None,
            validation_error: None,
        };

        assert!(!config.validate());
        assert!(!config.is_valid());
        assert!(config.get_validation_error().unwrap().contains("Invalid status code"));
    }

    #[test]
    fn test_response_rewrite_config_validate_status_range() {
        // Test lower bound
        let mut config = ResponseRewriteConfig {
            status_code: Some(100),
            ..Default::default()
        };
        assert!(config.validate());

        // Test upper bound
        let mut config = ResponseRewriteConfig {
            status_code: Some(599),
            ..Default::default()
        };
        assert!(config.validate());

        // Test below lower bound
        let mut config = ResponseRewriteConfig {
            status_code: Some(99),
            ..Default::default()
        };
        assert!(!config.validate());

        // Test above upper bound
        let mut config = ResponseRewriteConfig {
            status_code: Some(600),
            ..Default::default()
        };
        assert!(!config.validate());
    }

    #[test]
    fn test_response_rewrite_config_validate_empty_header_name() {
        let mut config = ResponseRewriteConfig {
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
            validation_error: None,
        };

        assert!(!config.validate());
        assert!(config.get_validation_error().unwrap().contains("cannot be empty"));
    }

    #[test]
    fn test_response_rewrite_config_validate_empty_rename() {
        // Empty 'from'
        let mut config = ResponseRewriteConfig {
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
            validation_error: None,
        };

        assert!(!config.validate());
        assert!(config.get_validation_error().unwrap().contains("'from'"));

        // Empty 'to'
        let mut config = ResponseRewriteConfig {
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
            validation_error: None,
        };

        assert!(!config.validate());
        assert!(config.get_validation_error().unwrap().contains("'to'"));
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
