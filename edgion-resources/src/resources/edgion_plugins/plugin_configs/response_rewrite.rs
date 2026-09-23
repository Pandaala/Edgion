//! Response Rewrite plugin configuration
//!
//! Rewrites responses before returning to client.
//! Supports status code and headers modification.

use http::HeaderValue;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::gwapi_types::{is_valid_header_token, HEADER_NAME_MAX_LEN};
use crate::response_header_policy::validate_response_header_mutation;

const INVALID_STATUS_CODE: &str = "statusCode must be between 200 and 599";
const INVALID_SET_NAME: &str = "headers.set name must be a valid HTTP header name of at most 256 bytes";
const INVALID_SET_VALUE: &str = "headers.set value must be a valid HTTP header value";
const INVALID_ADD_NAME: &str = "headers.add name must be a valid HTTP header name of at most 256 bytes";
const INVALID_ADD_VALUE: &str = "headers.add value must be a valid HTTP header value";
const INVALID_REMOVE_NAME: &str = "headers.remove name must be a valid HTTP header name of at most 256 bytes";
const INVALID_RENAME_SOURCE: &str = "headers.rename source must be a valid HTTP header name of at most 256 bytes";
const INVALID_RENAME_DESTINATION: &str =
    "headers.rename destination must be a valid HTTP header name of at most 256 bytes";
const INVALID_SELF_RENAME: &str = "headers.rename source and destination must differ";

fn is_valid_header_name(name: &str) -> bool {
    name.len() <= HEADER_NAME_MAX_LEN as usize && is_valid_header_token(name)
}

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
    /// New final HTTP status code (200-599).
    ///
    /// Overrides the upstream response status code.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 200, max = 599))]
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
        if let Some(status) = self.status_code {
            if !crate::http_status::is_valid_final_http_status(status) {
                return Some(INVALID_STATUS_CODE.to_string());
            }
        }

        if let Some(ref headers) = self.headers {
            if let Some(ref set_headers) = headers.set {
                for entry in set_headers {
                    if !is_valid_header_name(&entry.name) {
                        return Some(INVALID_SET_NAME.to_string());
                    }
                    if let Err(error) = validate_response_header_mutation(&entry.name) {
                        return Some(error.to_string());
                    }
                    if HeaderValue::from_str(&entry.value).is_err() {
                        return Some(INVALID_SET_VALUE.to_string());
                    }
                }
            }

            if let Some(ref add_headers) = headers.add {
                for entry in add_headers {
                    if !is_valid_header_name(&entry.name) {
                        return Some(INVALID_ADD_NAME.to_string());
                    }
                    if let Err(error) = validate_response_header_mutation(&entry.name) {
                        return Some(error.to_string());
                    }
                    if HeaderValue::from_str(&entry.value).is_err() {
                        return Some(INVALID_ADD_VALUE.to_string());
                    }
                }
            }

            if let Some(ref remove_headers) = headers.remove {
                for name in remove_headers {
                    if !is_valid_header_name(name) {
                        return Some(INVALID_REMOVE_NAME.to_string());
                    }
                    if let Err(error) = validate_response_header_mutation(name) {
                        return Some(error.to_string());
                    }
                }
            }

            if let Some(ref rename_headers) = headers.rename {
                for entry in rename_headers {
                    if !is_valid_header_name(&entry.from) {
                        return Some(INVALID_RENAME_SOURCE.to_string());
                    }
                    if !is_valid_header_name(&entry.to) {
                        return Some(INVALID_RENAME_DESTINATION.to_string());
                    }
                    if let Err(error) = validate_response_header_mutation(&entry.from)
                        .and_then(|()| validate_response_header_mutation(&entry.to))
                    {
                        return Some(error.to_string());
                    }
                    if entry.from.eq_ignore_ascii_case(&entry.to) {
                        return Some(INVALID_SELF_RENAME.to_string());
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

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for ResponseRewriteConfig {
    fn validation_error(&self) -> Option<String> {
        self.check_fields()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const PROTECTED_HEADER_ERROR: &str = "response header mutation targets a protected protocol field";

    fn config_with_headers(headers: ResponseHeaderActions) -> ResponseRewriteConfig {
        ResponseRewriteConfig {
            status_code: None,
            headers: Some(headers),
        }
    }

    fn header(name: impl Into<String>, value: impl Into<String>) -> ResponseHeaderEntry {
        ResponseHeaderEntry {
            name: name.into(),
            value: value.into(),
        }
    }

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
            status_code: Some(600),
            headers: None,
        };

        assert_eq!(config.check_fields().as_deref(), Some(INVALID_STATUS_CODE));
    }

    #[test]
    fn test_response_rewrite_config_validate_status_range() {
        // Informational responses cannot replace a final response.
        let config = ResponseRewriteConfig {
            status_code: Some(100),
            ..Default::default()
        };
        assert!(config.check_fields().is_some());

        // Test lower bound
        let config = ResponseRewriteConfig {
            status_code: Some(200),
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
    fn header_name_length_boundary_applies_to_every_action_position() {
        let max_name = "x".repeat(HEADER_NAME_MAX_LEN as usize);
        let other_max_name = format!("y{}", "x".repeat(HEADER_NAME_MAX_LEN as usize - 1));
        let valid = config_with_headers(ResponseHeaderActions {
            set: Some(vec![header(&max_name, "set")]),
            add: Some(vec![header(&other_max_name, "add")]),
            remove: Some(vec![max_name.clone()]),
            rename: Some(vec![HeaderRename {
                from: max_name.clone(),
                to: other_max_name,
            }]),
        });
        assert_eq!(valid.check_fields(), None);

        let too_long = "x".repeat(HEADER_NAME_MAX_LEN as usize + 1);
        let cases = [
            (
                config_with_headers(ResponseHeaderActions {
                    set: Some(vec![header(&too_long, "value")]),
                    add: None,
                    remove: None,
                    rename: None,
                }),
                INVALID_SET_NAME,
            ),
            (
                config_with_headers(ResponseHeaderActions {
                    set: None,
                    add: Some(vec![header(&too_long, "value")]),
                    remove: None,
                    rename: None,
                }),
                INVALID_ADD_NAME,
            ),
            (
                config_with_headers(ResponseHeaderActions {
                    set: None,
                    add: None,
                    remove: Some(vec![too_long.clone()]),
                    rename: None,
                }),
                INVALID_REMOVE_NAME,
            ),
            (
                config_with_headers(ResponseHeaderActions {
                    set: None,
                    add: None,
                    remove: None,
                    rename: Some(vec![HeaderRename {
                        from: too_long.clone(),
                        to: "X-New".to_string(),
                    }]),
                }),
                INVALID_RENAME_SOURCE,
            ),
            (
                config_with_headers(ResponseHeaderActions {
                    set: None,
                    add: None,
                    remove: None,
                    rename: Some(vec![HeaderRename {
                        from: "X-Old".to_string(),
                        to: too_long,
                    }]),
                }),
                INVALID_RENAME_DESTINATION,
            ),
        ];
        for (config, expected) in cases {
            assert_eq!(config.check_fields().as_deref(), Some(expected));
        }
    }

    #[test]
    fn invalid_header_tokens_are_classified_by_action_position() {
        let cases = [
            (
                config_with_headers(ResponseHeaderActions {
                    set: Some(vec![header("bad name", "value")]),
                    add: None,
                    remove: None,
                    rename: None,
                }),
                INVALID_SET_NAME,
            ),
            (
                config_with_headers(ResponseHeaderActions {
                    set: None,
                    add: Some(vec![header("bad:name", "value")]),
                    remove: None,
                    rename: None,
                }),
                INVALID_ADD_NAME,
            ),
            (
                config_with_headers(ResponseHeaderActions {
                    set: None,
                    add: None,
                    remove: Some(vec![String::new()]),
                    rename: None,
                }),
                INVALID_REMOVE_NAME,
            ),
            (
                config_with_headers(ResponseHeaderActions {
                    set: None,
                    add: None,
                    remove: None,
                    rename: Some(vec![HeaderRename {
                        from: "bad(name".to_string(),
                        to: "X-New".to_string(),
                    }]),
                }),
                INVALID_RENAME_SOURCE,
            ),
            (
                config_with_headers(ResponseHeaderActions {
                    set: None,
                    add: None,
                    remove: None,
                    rename: Some(vec![HeaderRename {
                        from: "X-Old".to_string(),
                        to: "bad name".to_string(),
                    }]),
                }),
                INVALID_RENAME_DESTINATION,
            ),
        ];
        for (config, expected) in cases {
            assert_eq!(config.check_fields().as_deref(), Some(expected));
        }
    }

    #[test]
    fn header_value_errors_are_fixed_and_do_not_echo_values() {
        let secret = "Bearer top-secret\r\nInjected: yes";
        let set = config_with_headers(ResponseHeaderActions {
            set: Some(vec![header("X-Token", secret)]),
            add: None,
            remove: None,
            rename: None,
        });
        let add = config_with_headers(ResponseHeaderActions {
            set: None,
            add: Some(vec![header("X-Token", secret)]),
            remove: None,
            rename: None,
        });

        let set_error = set.check_fields().unwrap();
        assert_eq!(set_error, INVALID_SET_VALUE);
        assert!(!set_error.contains("top-secret"));
        let add_error = add.check_fields().unwrap();
        assert_eq!(add_error, INVALID_ADD_VALUE);
        assert!(!add_error.contains("top-secret"));
    }

    #[test]
    fn framing_destinations_and_case_insensitive_self_renames_are_rejected() {
        let cases = [
            (
                config_with_headers(ResponseHeaderActions {
                    set: Some(vec![header("Content-Length", "1")]),
                    add: None,
                    remove: None,
                    rename: None,
                }),
                PROTECTED_HEADER_ERROR,
            ),
            (
                config_with_headers(ResponseHeaderActions {
                    set: None,
                    add: Some(vec![header("TRANSFER-ENCODING", "chunked")]),
                    remove: None,
                    rename: None,
                }),
                PROTECTED_HEADER_ERROR,
            ),
            (
                config_with_headers(ResponseHeaderActions {
                    set: None,
                    add: None,
                    remove: None,
                    rename: Some(vec![HeaderRename {
                        from: "X-Length".to_string(),
                        to: "content-length".to_string(),
                    }]),
                }),
                PROTECTED_HEADER_ERROR,
            ),
            (
                config_with_headers(ResponseHeaderActions {
                    set: None,
                    add: None,
                    remove: None,
                    rename: Some(vec![HeaderRename {
                        from: "X-Old".to_string(),
                        to: "x-old".to_string(),
                    }]),
                }),
                INVALID_SELF_RENAME,
            ),
        ];
        for (config, expected) in cases {
            assert_eq!(config.check_fields().as_deref(), Some(expected));
        }

        let framing_sources = config_with_headers(ResponseHeaderActions {
            set: None,
            add: None,
            remove: Some(vec!["Transfer-Encoding".to_string()]),
            rename: Some(vec![HeaderRename {
                from: "Content-Length".to_string(),
                to: "X-Upstream-Length".to_string(),
            }]),
        });
        assert_eq!(framing_sources.check_fields().as_deref(), Some(PROTECTED_HEADER_ERROR));
    }

    #[test]
    fn validation_error_priority_is_stable() {
        let invalid_value = "secret\r\nInjected: yes";
        let config = ResponseRewriteConfig {
            status_code: Some(600),
            headers: Some(ResponseHeaderActions {
                set: Some(vec![header("bad name", invalid_value)]),
                add: Some(vec![header("Content-Length", invalid_value)]),
                remove: Some(vec![String::new()]),
                rename: Some(vec![HeaderRename {
                    from: String::new(),
                    to: "Transfer-Encoding".to_string(),
                }]),
            }),
        };
        assert_eq!(config.check_fields().as_deref(), Some(INVALID_STATUS_CODE));

        let set_name_first = ResponseRewriteConfig {
            status_code: None,
            ..config.clone()
        };
        assert_eq!(set_name_first.check_fields().as_deref(), Some(INVALID_SET_NAME));

        let framing_before_value = config_with_headers(ResponseHeaderActions {
            set: Some(vec![header("Content-Length", invalid_value)]),
            add: None,
            remove: None,
            rename: None,
        });
        assert_eq!(
            framing_before_value.check_fields().as_deref(),
            Some(PROTECTED_HEADER_ERROR)
        );

        let add_before_remove = config_with_headers(ResponseHeaderActions {
            set: Some(vec![header("X-Set", "valid")]),
            add: Some(vec![header("Transfer-Encoding", invalid_value)]),
            remove: Some(vec![String::new()]),
            rename: Some(vec![HeaderRename {
                from: String::new(),
                to: String::new(),
            }]),
        });
        assert_eq!(
            add_before_remove.check_fields().as_deref(),
            Some(PROTECTED_HEADER_ERROR)
        );

        let remove_before_rename = config_with_headers(ResponseHeaderActions {
            set: None,
            add: None,
            remove: Some(vec![String::new()]),
            rename: Some(vec![HeaderRename {
                from: String::new(),
                to: String::new(),
            }]),
        });
        assert_eq!(
            remove_before_rename.check_fields().as_deref(),
            Some(INVALID_REMOVE_NAME)
        );

        let rename_source_before_destination = config_with_headers(ResponseHeaderActions {
            set: None,
            add: None,
            remove: None,
            rename: Some(vec![HeaderRename {
                from: String::new(),
                to: String::new(),
            }]),
        });
        assert_eq!(
            rename_source_before_destination.check_fields().as_deref(),
            Some(INVALID_RENAME_SOURCE)
        );

        let rename_framing_before_self = config_with_headers(ResponseHeaderActions {
            set: None,
            add: None,
            remove: None,
            rename: Some(vec![HeaderRename {
                from: "Content-Length".to_string(),
                to: "content-length".to_string(),
            }]),
        });
        assert_eq!(
            rename_framing_before_self.check_fields().as_deref(),
            Some(PROTECTED_HEADER_ERROR)
        );
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
    use crate::resources::edgion_plugins::PluginValidation;

    #[test]
    fn validation_error_returns_field_error_without_validate_call() {
        let config = ResponseRewriteConfig {
            status_code: Some(600),
            headers: None,
        };

        assert_eq!(config.validation_error().as_deref(), Some(INVALID_STATUS_CODE));
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
        assert_eq!(config.validation_error().as_deref(), Some(INVALID_SET_NAME));
    }
}
