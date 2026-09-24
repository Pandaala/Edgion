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
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/response_rewrite_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/response_rewrite_test/validation_trait_tests.rs"]
mod validation_trait_tests;
