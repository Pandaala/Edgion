use crate::gwapi_types::{is_valid_header_token, HEADER_NAME_MAX_LEN};
use crate::response_header_policy::validate_response_header_mutation;
use http::HeaderValue;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Mock plugin configuration
///
/// This plugin returns predefined mock responses without forwarding requests to upstream.
/// Useful for API prototyping, testing, and simulating error conditions.
///
/// ## Usage Example:
/// ```yaml
/// mock:
///   statusCode: 200
///   body: '{"message":"Hello, World!"}'
///   contentType: "application/json"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MockConfig {
    /// HTTP status code to return (default: 200)
    ///
    /// Valid range: 200-599
    #[serde(default = "default_status_code")]
    #[schemars(range(min = 200, max = 599))]
    pub status_code: u16,

    /// Response body content
    ///
    /// Can be any string content (JSON, XML, plain text, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,

    /// Response headers to set
    ///
    /// Example: {"X-Custom-Header": "value", "X-Request-ID": "12345"}
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,

    /// Content-Type header (default: "application/json")
    #[serde(default = "default_content_type")]
    pub content_type: String,

    /// Delay in milliseconds before responding
    ///
    /// Useful for simulating slow network or rate limiting scenarios
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delay: Option<u64>,

    /// Whether to terminate the request after execution
    ///
    /// - `true` (default): Return the mock response and stop processing
    /// - `false`: Execute (e.g., set headers) and continue to next plugin/upstream
    #[serde(default = "default_true")]
    pub terminate: bool,
}

fn default_status_code() -> u16 {
    200
}

fn default_content_type() -> String {
    "application/json".to_string()
}

fn default_true() -> bool {
    true
}

const MAX_DELAY_MS: u64 = 30_000;

const INVALID_CONTENT_TYPE: &str = "mock.contentType must be a valid HTTP header value";
const INVALID_HEADER_NAME: &str = "mock.headers contains an invalid HTTP header name";
const INVALID_HEADER_VALUE: &str = "mock.headers contains an invalid HTTP header value";
const CONTENT_TYPE_IN_HEADERS: &str = "mock.headers must not contain Content-Type; use contentType instead";
const DUPLICATE_HEADER_NAME: &str = "mock.headers must not contain case-insensitive duplicate names";

impl Default for MockConfig {
    fn default() -> Self {
        MockConfig {
            status_code: default_status_code(),
            body: None,
            headers: None,
            content_type: default_content_type(),
            delay: None,
            terminate: true,
        }
    }
}

impl MockConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if !crate::http_status::is_valid_final_http_status(self.status_code) {
            return Err(format!(
                "Invalid status code: {}. Must be between 200 and 599",
                self.status_code
            ));
        }
        if let Some(delay) = self.delay {
            if delay > MAX_DELAY_MS {
                return Err(format!("delay must not exceed {}ms, got {}ms", MAX_DELAY_MS, delay));
            }
        }

        if HeaderValue::try_from(self.content_type.as_str()).is_err() {
            return Err(INVALID_CONTENT_TYPE.to_string());
        }

        let Some(headers) = self.headers.as_ref() else {
            return Ok(());
        };

        if headers
            .keys()
            .any(|name| name.len() > HEADER_NAME_MAX_LEN as usize || !is_valid_header_token(name))
        {
            return Err(INVALID_HEADER_NAME.to_string());
        }

        if headers
            .values()
            .any(|value| HeaderValue::try_from(value.as_str()).is_err())
        {
            return Err(INVALID_HEADER_VALUE.to_string());
        }

        if headers.keys().any(|name| name.eq_ignore_ascii_case("content-type")) {
            return Err(CONTENT_TYPE_IN_HEADERS.to_string());
        }

        for name in headers.keys() {
            validate_response_header_mutation(name).map_err(str::to_owned)?;
        }

        let mut normalized_names = HashSet::with_capacity(headers.len());
        if headers
            .keys()
            .any(|name| !normalized_names.insert(name.to_ascii_lowercase()))
        {
            return Err(DUPLICATE_HEADER_NAME.to_string());
        }

        Ok(())
    }

    /// Create a new MockConfig with status code and body
    pub fn new(status_code: u16, body: String) -> Self {
        MockConfig {
            status_code,
            body: Some(body),
            headers: None,
            content_type: default_content_type(),
            delay: None,
            terminate: true,
        }
    }
}

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for MockConfig {
    fn validation_error(&self) -> Option<String> {
        self.validate().err()
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/mock_test.rs"]
mod tests;
