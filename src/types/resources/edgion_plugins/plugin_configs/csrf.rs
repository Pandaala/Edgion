use serde::{Deserialize, Serialize};
use schemars::JsonSchema;

/// CSRF (Cross-Site Request Forgery) protection plugin configuration
///
/// This plugin protects against CSRF attacks by validating tokens in requests
/// and setting CSRF tokens in response cookies.
///
/// ## How it works:
/// 1. On ResponseHeader stage: generates and sets a CSRF token cookie
/// 2. On Request stage: validates that the token in header matches the cookie token
///
/// ## Usage Example:
/// ```yaml
/// csrf:
///   key: "my-secret-key-32-chars-long!!"
///   expires: 7200  # 2 hours
///   name: "apisix-csrf-token"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CsrfConfig {
    /// Secret key used for signing CSRF tokens (required)
    ///
    /// This key is used to generate and verify token signatures.
    /// Keep this secret and use a strong random value.
    pub key: String,

    /// Token expiration time in seconds (default: 7200 = 2 hours)
    ///
    /// After this time, tokens will be considered expired and rejected.
    #[serde(default = "default_expires")]
    pub expires: i64,

    /// Token name used in both cookie and header (default: "apisix-csrf-token")
    ///
    /// The client must send the token value in a request header with this name.
    #[serde(default = "default_name")]
    pub name: String,
}

fn default_expires() -> i64 {
    7200 // 2 hours
}

fn default_name() -> String {
    "apisix-csrf-token".to_string()
}

impl Default for CsrfConfig {
    fn default() -> Self {
        CsrfConfig {
            key: String::new(),
            expires: default_expires(),
            name: default_name(),
        }
    }
}

impl CsrfConfig {
    /// Validate the configuration
    ///
    /// Returns an error if the key is empty (required field).
    pub fn validate(&self) -> Result<(), String> {
        if self.key.is_empty() {
            return Err("CSRF config validation failed: 'key' is required and cannot be empty".to_string());
        }
        Ok(())
    }

    /// Create a new CsrfConfig with the given key
    pub fn new(key: String) -> Self {
        CsrfConfig {
            key,
            expires: default_expires(),
            name: default_name(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = CsrfConfig::default();
        assert_eq!(config.key, "");
        assert_eq!(config.expires, 7200);
        assert_eq!(config.name, "apisix-csrf-token");
    }

    #[test]
    fn test_new_config() {
        let config = CsrfConfig::new("test-secret-key".to_string());
        assert_eq!(config.key, "test-secret-key");
        assert_eq!(config.expires, 7200);
        assert_eq!(config.name, "apisix-csrf-token");
    }

    #[test]
    fn test_validate_success() {
        let config = CsrfConfig::new("valid-key".to_string());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_empty_key() {
        let config = CsrfConfig::default();
        assert!(config.validate().is_err());
        assert!(config.validate().unwrap_err().contains("key"));
    }

    #[test]
    fn test_serde_serialization() {
        let config = CsrfConfig {
            key: "my-secret".to_string(),
            expires: 3600,
            name: "my-csrf-token".to_string(),
        };

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("my-secret"));
        assert!(json.contains("3600"));
        assert!(json.contains("my-csrf-token"));
    }

    #[test]
    fn test_serde_deserialization() {
        let json = r#"{"key":"test-key","expires":1800,"name":"custom-token"}"#;
        let config: CsrfConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.key, "test-key");
        assert_eq!(config.expires, 1800);
        assert_eq!(config.name, "custom-token");
    }

    #[test]
    fn test_serde_deserialization_with_defaults() {
        let json = r#"{"key":"test-key"}"#;
        let config: CsrfConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.key, "test-key");
        assert_eq!(config.expires, 7200); // default
        assert_eq!(config.name, "apisix-csrf-token"); // default
    }
}
