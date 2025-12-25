use serde::{Deserialize, Serialize};
use schemars::JsonSchema;
use std::collections::HashMap;

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
    /// Valid range: 100-599
    #[serde(default = "default_status_code")]
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
}

fn default_status_code() -> u16 {
    200
}

fn default_content_type() -> String {
    "application/json".to_string()
}

impl Default for MockConfig {
    fn default() -> Self {
        MockConfig {
            status_code: default_status_code(),
            body: None,
            headers: None,
            content_type: default_content_type(),
            delay: None,
        }
    }
}

impl MockConfig {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.status_code < 100 || self.status_code >= 600 {
            return Err(format!("Invalid status code: {}. Must be between 100 and 599", self.status_code));
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let config = MockConfig::new(200, "OK".to_string());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_invalid_status() {
        let mut config = MockConfig::default();
        config.status_code = 99;
        assert!(config.validate().is_err());

        config.status_code = 600;
        assert!(config.validate().is_err());
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
