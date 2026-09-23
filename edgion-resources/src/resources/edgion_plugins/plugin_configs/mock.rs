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
mod tests {
    use super::*;
    const PROTECTED_HEADER_ERROR: &str = "response header mutation targets a protected protocol field";

    fn headers(entries: &[(&str, &str)]) -> Option<HashMap<String, String>> {
        Some(
            entries
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect(),
        )
    }

    #[test]
    fn test_default_config() {
        let config = MockConfig::default();
        assert_eq!(config.status_code, 200);
        assert_eq!(config.content_type, "application/json");
        assert!(config.body.is_none());
        assert!(config.headers.is_none());
        assert!(config.delay.is_none());
    }

    #[test]
    fn test_new_config() {
        let config = MockConfig::new(404, r#"{"error":"Not Found"}"#.to_string());
        assert_eq!(config.status_code, 404);
        assert_eq!(config.body, Some(r#"{"error":"Not Found"}"#.to_string()));
    }

    #[test]
    fn test_validate_success() {
        for status_code in [200, 599] {
            let config = MockConfig {
                headers: headers(&[("X-Mock", "yes"), ("Cache-Control", "no-cache")]),
                content_type: "application/json; charset=utf-8".to_string(),
                ..MockConfig::new(status_code, "OK".to_string())
            };
            assert!(config.validate().is_ok(), "{status_code}");
        }
    }

    #[test]
    fn test_validate_invalid_status() {
        for status_code in [100, 101, 199, 600] {
            let config = MockConfig {
                status_code,
                ..Default::default()
            };
            assert!(config.validate().is_err(), "{status_code}");
        }
    }

    #[test]
    fn status_code_schema_matches_final_response_range() {
        let schema = serde_json::to_value(schemars::schema_for!(MockConfig)).unwrap();
        let status = &schema["properties"]["statusCode"];
        assert_eq!(status["minimum"].as_u64(), Some(200));
        assert_eq!(status["maximum"].as_u64(), Some(599));
    }

    #[test]
    fn test_validate_delay_limit() {
        let config = MockConfig {
            delay: Some(MAX_DELAY_MS),
            ..Default::default()
        };
        assert!(config.validate().is_ok());

        let config = MockConfig {
            delay: Some(MAX_DELAY_MS + 1),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_header_names() {
        for invalid_name in ["bad header".to_string(), "x".repeat(HEADER_NAME_MAX_LEN as usize + 1)] {
            let config = MockConfig {
                headers: Some(HashMap::from([(invalid_name, "value".to_string())])),
                ..Default::default()
            };
            assert_eq!(config.validate().unwrap_err(), INVALID_HEADER_NAME);
        }
    }

    #[test]
    fn test_validate_header_values_without_echoing_input() {
        let sentinel = "private-value";
        let config = MockConfig {
            headers: headers(&[("X-Mock", &format!("{sentinel}\r\ninjected: true"))]),
            ..Default::default()
        };

        let error = config.validate().unwrap_err();
        assert_eq!(error, INVALID_HEADER_VALUE);
        assert!(!error.contains(sentinel));
    }

    #[test]
    fn test_validate_content_type_without_echoing_input() {
        let sentinel = "private-content-type";
        let config = MockConfig {
            content_type: format!("text/plain\r\n{sentinel}"),
            ..Default::default()
        };

        let error = config.validate().unwrap_err();
        assert_eq!(error, INVALID_CONTENT_TYPE);
        assert!(!error.contains(sentinel));
    }

    #[test]
    fn test_validate_reserved_headers_case_insensitively() {
        for (name, expected) in [
            ("cOnTeNt-TyPe", CONTENT_TYPE_IN_HEADERS),
            ("CONTENT-length", PROTECTED_HEADER_ERROR),
            ("Transfer-Encoding", PROTECTED_HEADER_ERROR),
        ] {
            let config = MockConfig {
                headers: headers(&[(name, "value")]),
                ..Default::default()
            };
            assert_eq!(config.validate().unwrap_err(), expected);
        }
    }

    #[test]
    fn test_validate_duplicate_headers_case_insensitively() {
        let config = MockConfig {
            headers: headers(&[("X-Mock", "one"), ("x-mock", "two")]),
            ..Default::default()
        };
        assert_eq!(config.validate().unwrap_err(), DUPLICATE_HEADER_NAME);
    }

    #[test]
    fn test_validation_priority_is_deterministic() {
        let mut config = MockConfig {
            status_code: 99,
            delay: Some(MAX_DELAY_MS + 1),
            content_type: "bad\r\ncontent-type".to_string(),
            headers: headers(&[("bad header", "bad\r\nvalue")]),
            ..Default::default()
        };
        assert_eq!(
            config.validate().unwrap_err(),
            "Invalid status code: 99. Must be between 200 and 599"
        );

        config.status_code = 200;
        assert_eq!(
            config.validate().unwrap_err(),
            "delay must not exceed 30000ms, got 30001ms"
        );

        config.delay = None;
        assert_eq!(config.validate().unwrap_err(), INVALID_CONTENT_TYPE);

        config.content_type = default_content_type();
        assert_eq!(config.validate().unwrap_err(), INVALID_HEADER_NAME);

        config.headers = headers(&[("X-Mock", "bad\r\nvalue")]);
        assert_eq!(config.validate().unwrap_err(), INVALID_HEADER_VALUE);

        config.headers = headers(&[("Content-Type", "text/plain")]);
        assert_eq!(config.validate().unwrap_err(), CONTENT_TYPE_IN_HEADERS);

        config.headers = headers(&[("Content-Length", "1")]);
        assert_eq!(config.validate().unwrap_err(), PROTECTED_HEADER_ERROR);

        config.headers = headers(&[("Transfer-Encoding", "chunked")]);
        assert_eq!(config.validate().unwrap_err(), PROTECTED_HEADER_ERROR);

        config.headers = headers(&[("X-Mock", "one"), ("x-mock", "two")]);
        assert_eq!(config.validate().unwrap_err(), DUPLICATE_HEADER_NAME);
    }

    #[test]
    fn test_serde_serialization() {
        let mut headers = HashMap::new();
        headers.insert("X-Custom".to_string(), "value".to_string());

        let config = MockConfig {
            status_code: 201,
            body: Some(r#"{"created":true}"#.to_string()),
            headers: Some(headers),
            content_type: "application/json".to_string(),
            delay: Some(100),
            terminate: true,
        };

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("201"));
        assert!(json.contains("created"));
        assert!(json.contains("X-Custom"));
        assert!(json.contains("100"));
    }

    #[test]
    fn test_serde_deserialization() {
        let json = r#"{
            "statusCode": 500,
            "body": "Internal Server Error",
            "contentType": "text/plain",
            "delay": 50
        }"#;

        let config: MockConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.status_code, 500);
        assert_eq!(config.body, Some("Internal Server Error".to_string()));
        assert_eq!(config.content_type, "text/plain");
        assert_eq!(config.delay, Some(50));
    }

    #[test]
    fn test_serde_with_defaults() {
        let json = r#"{"body": "test"}"#;
        let config: MockConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.status_code, 200); // default
        assert_eq!(config.content_type, "application/json"); // default
        assert_eq!(config.body, Some("test".to_string()));
    }
}
