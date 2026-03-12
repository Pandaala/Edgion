//! ForwardAuth plugin configuration
//!
//! Sends the original request metadata to an external authentication service.
//! If the auth service responds with 2xx, the request is forwarded to upstream.
//! Otherwise, the auth service's response is returned to the client.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// ForwardAuth plugin configuration.
///
/// Sends the original request metadata to an external authentication service.
/// If the auth service responds with 2xx, the request is forwarded to upstream.
/// Otherwise, the auth service's response is returned to the client.
///
/// ## Examples
///
/// ### Basic: forward all headers
/// ```yaml
/// uri: "http://auth-service:8080/verify"
/// upstreamHeaders:
///   - X-User-ID
///   - X-User-Role
/// ```
///
/// ### Selective: forward only specific headers
/// ```yaml
/// uri: "https://auth.example.com/api/verify"
/// requestMethod: POST
/// timeoutMs: 5000
/// requestHeaders:
///   - Authorization
///   - Cookie
/// upstreamHeaders:
///   - X-User-ID
/// clientHeaders:
///   - WWW-Authenticate
/// successStatusCodes: [200, 204]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ForwardAuthConfig {
    /// Auth service URL (required).
    /// Example: "http://auth-service:8080/verify"
    pub uri: String,

    /// HTTP method to use for the auth request.
    /// Default: GET (most auth services use GET)
    #[serde(default = "default_method")]
    pub request_method: String,

    /// Headers to forward from the original request to the auth service.
    ///
    /// Behavior:
    /// - If empty/None: forward ALL request headers (skipping hop-by-hop)
    /// - If specified: forward ONLY the listed headers
    ///
    /// In both cases, hop-by-hop headers are always excluded.
    #[serde(default)]
    pub request_headers: Option<Vec<String>>,

    /// Headers to copy from auth service's response back to the original request
    /// (forwarded to upstream).
    ///
    /// Useful for passing identity info: e.g., ["X-User-ID", "X-User-Role"]
    #[serde(default)]
    pub upstream_headers: Vec<String>,

    /// Headers to copy from auth service's response to the client response
    /// (only when auth fails, i.e., non-2xx response).
    ///
    /// Useful for passing error details: e.g., ["X-Auth-Error", "WWW-Authenticate"]
    #[serde(default)]
    pub client_headers: Vec<String>,

    /// Request timeout in milliseconds.
    /// Default: 10000 (10 seconds)
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,

    /// Status codes from auth service that are considered "success" (forward to upstream).
    /// Default: any 2xx status code
    /// Example: [200, 201, 204]
    #[serde(default)]
    pub success_status_codes: Option<Vec<u16>>,

    /// Whether to allow the request to proceed when the auth service is unavailable
    /// (network error, timeout, etc.).
    ///
    /// - false (default): Return error response (status_on_error) when auth service is down
    /// - true: Skip authentication and forward to upstream (degraded mode)
    ///
    /// This is useful for non-critical auth checks where availability is more important
    /// than strict authentication. Comparable to APISIX's `allow_degradation`.
    #[serde(default)]
    pub allow_degradation: bool,

    /// HTTP status code to return when the auth service is unreachable or returns a network error.
    /// Only effective when allow_degradation is false.
    /// Default: 503 (Service Unavailable)
    /// Range: 200-599
    ///
    /// Comparable to APISIX's `status_on_error`.
    #[serde(default = "default_status_on_error")]
    pub status_on_error: u16,

    /// Remove Authorization header from the upstream request after successful auth.
    /// Default: false.
    #[serde(default)]
    pub hide_credentials: bool,

    /// Delay in milliseconds before returning an authentication failure response.
    /// Increases the time cost for brute-force / credential-stuffing attacks.
    /// Default: 0 (no delay).
    #[serde(default)]
    pub auth_failure_delay_ms: u64,
}

fn default_method() -> String {
    "GET".to_string()
}

fn default_timeout_ms() -> u64 {
    10000
}

fn default_status_on_error() -> u16 {
    503
}

impl Default for ForwardAuthConfig {
    fn default() -> Self {
        Self {
            uri: String::new(),
            request_method: default_method(),
            request_headers: None,
            upstream_headers: Vec::new(),
            client_headers: Vec::new(),
            timeout_ms: default_timeout_ms(),
            success_status_codes: None,
            allow_degradation: false,
            status_on_error: default_status_on_error(),
            hide_credentials: false,
            auth_failure_delay_ms: 0,
        }
    }
}

impl ForwardAuthConfig {
    /// Validate the configuration and return an error message if invalid.
    pub fn get_validation_error(&self) -> Option<&str> {
        if self.uri.is_empty() {
            return Some("uri is required");
        }
        // Validate URI format (must start with http:// or https://)
        if !self.uri.starts_with("http://") && !self.uri.starts_with("https://") {
            return Some("uri must start with http:// or https://");
        }
        // Validate method
        let method_upper = self.request_method.to_uppercase();
        if !matches!(
            method_upper.as_str(),
            "GET" | "POST" | "PUT" | "DELETE" | "PATCH" | "HEAD" | "OPTIONS"
        ) {
            return Some("requestMethod must be a valid HTTP method");
        }
        // Validate timeout
        if self.timeout_ms == 0 {
            return Some("timeoutMs must be greater than 0");
        }
        // Validate status_on_error
        if !(200..=599).contains(&self.status_on_error) {
            return Some("statusOnError must be between 200 and 599");
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ForwardAuthConfig::default();
        assert_eq!(config.request_method, "GET");
        assert_eq!(config.timeout_ms, 10000);
        assert!(config.request_headers.is_none());
        assert!(config.upstream_headers.is_empty());
        assert!(config.client_headers.is_empty());
        assert!(config.success_status_codes.is_none());
        assert!(!config.allow_degradation);
        assert_eq!(config.status_on_error, 503);
    }

    #[test]
    fn test_validation_empty_uri() {
        let config = ForwardAuthConfig::default();
        assert_eq!(config.get_validation_error(), Some("uri is required"));
    }

    #[test]
    fn test_validation_invalid_uri_scheme() {
        let config = ForwardAuthConfig {
            uri: "ftp://auth-service/verify".to_string(),
            ..Default::default()
        };
        assert_eq!(
            config.get_validation_error(),
            Some("uri must start with http:// or https://")
        );
    }

    #[test]
    fn test_validation_invalid_method() {
        let config = ForwardAuthConfig {
            uri: "http://auth-service/verify".to_string(),
            request_method: "INVALID".to_string(),
            ..Default::default()
        };
        assert_eq!(
            config.get_validation_error(),
            Some("requestMethod must be a valid HTTP method")
        );
    }

    #[test]
    fn test_validation_zero_timeout() {
        let config = ForwardAuthConfig {
            uri: "http://auth-service/verify".to_string(),
            timeout_ms: 0,
            ..Default::default()
        };
        assert_eq!(config.get_validation_error(), Some("timeoutMs must be greater than 0"));
    }

    #[test]
    fn test_validation_valid_config() {
        let config = ForwardAuthConfig {
            uri: "http://auth-service:8080/verify".to_string(),
            ..Default::default()
        };
        assert!(config.get_validation_error().is_none());
    }

    #[test]
    fn test_validation_https_uri() {
        let config = ForwardAuthConfig {
            uri: "https://auth.example.com/api/verify".to_string(),
            ..Default::default()
        };
        assert!(config.get_validation_error().is_none());
    }

    #[test]
    fn test_deserialization_defaults() {
        let json = r#"{"uri": "http://auth-service/verify"}"#;
        let config: ForwardAuthConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.uri, "http://auth-service/verify");
        assert_eq!(config.request_method, "GET");
        assert_eq!(config.timeout_ms, 10000);
        assert!(config.request_headers.is_none());
    }

    #[test]
    fn test_deserialization_full() {
        let json = r#"{
            "uri": "https://auth.example.com/verify",
            "requestMethod": "POST",
            "timeoutMs": 5000,
            "requestHeaders": ["Authorization", "Cookie"],
            "upstreamHeaders": ["X-User-ID", "X-User-Role"],
            "clientHeaders": ["WWW-Authenticate"],
            "successStatusCodes": [200, 204],
            "allowDegradation": true,
            "statusOnError": 403
        }"#;
        let config: ForwardAuthConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.uri, "https://auth.example.com/verify");
        assert_eq!(config.request_method, "POST");
        assert_eq!(config.timeout_ms, 5000);
        assert_eq!(
            config.request_headers,
            Some(vec!["Authorization".to_string(), "Cookie".to_string()])
        );
        assert_eq!(config.upstream_headers, vec!["X-User-ID", "X-User-Role"]);
        assert_eq!(config.client_headers, vec!["WWW-Authenticate"]);
        assert_eq!(config.success_status_codes, Some(vec![200, 204]));
        assert!(config.allow_degradation);
        assert_eq!(config.status_on_error, 403);
    }

    #[test]
    fn test_deserialization_defaults_new_fields() {
        let json = r#"{"uri": "http://auth-service/verify"}"#;
        let config: ForwardAuthConfig = serde_json::from_str(json).unwrap();
        // New fields should have sensible defaults
        assert!(!config.allow_degradation);
        assert_eq!(config.status_on_error, 503);
    }

    #[test]
    fn test_validation_invalid_status_on_error() {
        let config = ForwardAuthConfig {
            uri: "http://auth-service/verify".to_string(),
            status_on_error: 600, // Invalid: out of range
            ..Default::default()
        };
        assert_eq!(
            config.get_validation_error(),
            Some("statusOnError must be between 200 and 599")
        );
    }

    #[test]
    fn test_validation_status_on_error_valid_range() {
        let config = ForwardAuthConfig {
            uri: "http://auth-service/verify".to_string(),
            status_on_error: 403,
            ..Default::default()
        };
        assert!(config.get_validation_error().is_none());
    }
}
