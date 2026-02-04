//! Proxy Rewrite plugin configuration
//!
//! Rewrites requests before forwarding to upstream services.
//! Supports URI, Host, Method, and Headers modification.

use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Proxy Rewrite plugin configuration
///
/// Rewrites requests before forwarding to upstream services.
/// All fields are optional - configure only what you need.
///
/// # Example
///
/// ```yaml
/// type: ProxyRewrite
/// config:
///   # URI rewrite (choose one: uri or regexUri)
///   uri: "/internal/api/v2"
///   # Or use regex:
///   # regexUri:
///   #   pattern: "^/api/v1/users/([0-9]+)"
///   #   replacement: "/user-service/$1"
///   
///   host: "backend.internal.svc"
///   
///   headers:
///     set:
///       - name: X-Api-Version
///         value: "v2"
///     remove:
///       - X-Debug
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProxyRewriteConfig {
    /// New upstream URI path.
    ///
    /// Supports variable substitution:
    /// - `$uri` - original request path
    /// - `$arg_<name>` - query parameter value
    ///
    /// Query string from original request is preserved automatically.
    /// When both `uri` and `regexUri` are configured, `uri` takes priority.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,

    /// Regex-based URI rewrite.
    ///
    /// Uses regex to match the original path and generates a new path using the replacement template.
    /// The replacement supports `$1`, `$2`, etc. for capture group references.
    ///
    /// Query string from original request is preserved automatically.
    ///
    /// # Example
    ///
    /// ```yaml
    /// regexUri:
    ///   pattern: "^/api/v1/users/([0-9]+)/profile"
    ///   replacement: "/user-service/$1"
    /// ```
    #[serde(skip_serializing_if = "Option::is_none")]
    pub regex_uri: Option<RegexUri>,

    /// Set Host request header.
    ///
    /// Note: Do not configure Host in `headers.set` - use this field instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,

    /// Rewrite HTTP method.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<HttpMethod>,

    /// Request header operations.
    ///
    /// Execution order: add -> set -> remove.
    /// Header values support variable substitution: `$uri`, `$arg_<name>`, `$1-$9` (only when regexUri matches).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<HeaderActions>,

    // ========== Runtime fields (not serialized) ==========
    /// Precompiled regex pattern.
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_regex: Option<Regex>,

    /// Configuration validation error.
    #[serde(skip)]
    #[schemars(skip)]
    pub validation_error: Option<String>,
}

/// Regex URI rewrite configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegexUri {
    /// Regex pattern to match the URI.
    pub pattern: String,

    /// Replacement template, supports `$1`, `$2`, etc. for capture group references.
    pub replacement: String,
}

/// HTTP method enumeration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
    Trace,
    Connect,
    Mkcol,
    Copy,
    Move,
    Propfind,
    Lock,
    Unlock,
}

/// A single header name-value pair.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HeaderEntry {
    /// Header name
    pub name: String,
    /// Header value (supports variable substitution)
    pub value: String,
}

/// Request header operations configuration.
///
/// Uses Vec instead of HashMap to preserve insertion order.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HeaderActions {
    /// Add request headers (append to existing values with comma separation).
    /// Headers are processed in order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub add: Option<Vec<HeaderEntry>>,

    /// Set request headers (overwrite existing values).
    /// Headers are processed in order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub set: Option<Vec<HeaderEntry>>,

    /// Remove request headers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remove: Option<Vec<String>>,
}

impl Default for ProxyRewriteConfig {
    fn default() -> Self {
        Self {
            uri: None,
            regex_uri: None,
            host: None,
            method: None,
            headers: None,
            compiled_regex: None,
            validation_error: None,
        }
    }
}

impl ProxyRewriteConfig {
    /// Precompile regex pattern and validate configuration.
    ///
    /// Returns true if configuration is valid, false otherwise.
    pub fn precompile(&mut self) -> bool {
        // Check for Host conflict in headers.set
        if self.host.is_some() {
            if let Some(ref headers) = self.headers {
                if let Some(ref set_headers) = headers.set {
                    for entry in set_headers {
                        if entry.name.eq_ignore_ascii_case("host") {
                            self.validation_error = Some(
                                "Conflict: 'host' field and 'headers.set' with Host header both configured. Use only 'host' field.".to_string()
                            );
                            return false;
                        }
                    }
                }
            }
        }

        // Compile regex if configured
        if let Some(ref regex_uri) = self.regex_uri {
            match Regex::new(&regex_uri.pattern) {
                Ok(regex) => {
                    self.compiled_regex = Some(regex);
                    self.validation_error = None;
                    true
                }
                Err(e) => {
                    self.validation_error = Some(format!("Invalid regex pattern: {}", e));
                    false
                }
            }
        } else {
            self.validation_error = None;
            true
        }
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
        self.uri.is_some()
            || self.regex_uri.is_some()
            || self.host.is_some()
            || self.method.is_some()
            || self.headers.is_some()
    }
}

impl HttpMethod {
    /// Convert to HTTP method string.
    pub fn as_str(&self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Head => "HEAD",
            HttpMethod::Options => "OPTIONS",
            HttpMethod::Trace => "TRACE",
            HttpMethod::Connect => "CONNECT",
            HttpMethod::Mkcol => "MKCOL",
            HttpMethod::Copy => "COPY",
            HttpMethod::Move => "MOVE",
            HttpMethod::Propfind => "PROPFIND",
            HttpMethod::Lock => "LOCK",
            HttpMethod::Unlock => "UNLOCK",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_rewrite_config_default() {
        let config = ProxyRewriteConfig::default();
        assert!(config.uri.is_none());
        assert!(config.regex_uri.is_none());
        assert!(config.host.is_none());
        assert!(config.method.is_none());
        assert!(config.headers.is_none());
        assert!(config.is_valid());
        assert!(!config.has_any_config());
    }

    #[test]
    fn test_proxy_rewrite_config_precompile_valid() {
        let mut config = ProxyRewriteConfig {
            regex_uri: Some(RegexUri {
                pattern: r"^/api/v1/users/(\d+)".to_string(),
                replacement: "/users/$1".to_string(),
            }),
            ..Default::default()
        };

        assert!(config.precompile());
        assert!(config.is_valid());
        assert!(config.compiled_regex.is_some());
    }

    #[test]
    fn test_proxy_rewrite_config_precompile_invalid() {
        let mut config = ProxyRewriteConfig {
            regex_uri: Some(RegexUri {
                pattern: r"[invalid".to_string(), // Invalid regex
                replacement: "/users/$1".to_string(),
            }),
            ..Default::default()
        };

        assert!(!config.precompile());
        assert!(!config.is_valid());
        assert!(config.get_validation_error().is_some());
    }

    #[test]
    fn test_host_conflict_detection() {
        let mut config = ProxyRewriteConfig {
            host: Some("backend.svc".to_string()),
            headers: Some(HeaderActions {
                add: None,
                set: Some(vec![HeaderEntry {
                    name: "Host".to_string(),
                    value: "other.svc".to_string(),
                }]),
                remove: None,
            }),
            ..Default::default()
        };

        assert!(!config.precompile());
        assert!(!config.is_valid());
        assert!(config.get_validation_error().unwrap().contains("Conflict"));
    }

    #[test]
    fn test_http_method_as_str() {
        assert_eq!(HttpMethod::Get.as_str(), "GET");
        assert_eq!(HttpMethod::Post.as_str(), "POST");
        assert_eq!(HttpMethod::Put.as_str(), "PUT");
        assert_eq!(HttpMethod::Delete.as_str(), "DELETE");
        assert_eq!(HttpMethod::Patch.as_str(), "PATCH");
    }

    #[test]
    fn test_proxy_rewrite_config_serde() {
        let json = r#"{
            "uri": "/new/path",
            "host": "backend.svc",
            "method": "POST",
            "headers": {
                "set": [
                    {"name": "X-Api-Version", "value": "v2"}
                ],
                "remove": ["X-Debug"]
            }
        }"#;

        let config: ProxyRewriteConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.uri, Some("/new/path".to_string()));
        assert_eq!(config.host, Some("backend.svc".to_string()));
        assert!(matches!(config.method, Some(HttpMethod::Post)));

        let headers = config.headers.as_ref().unwrap();
        let set_headers = headers.set.as_ref().unwrap();
        assert_eq!(set_headers.len(), 1);
        assert_eq!(set_headers[0].name, "X-Api-Version");
        assert_eq!(set_headers[0].value, "v2");
        assert!(headers.remove.as_ref().unwrap().contains(&"X-Debug".to_string()));
    }

    #[test]
    fn test_regex_uri_serde() {
        let json = r#"{
            "regexUri": {
                "pattern": "^/api/v1/(.*)",
                "replacement": "/internal/$1"
            }
        }"#;

        let config: ProxyRewriteConfig = serde_json::from_str(json).unwrap();
        let regex_uri = config.regex_uri.as_ref().unwrap();
        assert_eq!(regex_uri.pattern, "^/api/v1/(.*)");
        assert_eq!(regex_uri.replacement, "/internal/$1");
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

        let config: ProxyRewriteConfig = serde_json::from_str(json).unwrap();
        let set_headers = config.headers.as_ref().unwrap().set.as_ref().unwrap();
        assert_eq!(set_headers[0].name, "X-First");
        assert_eq!(set_headers[1].name, "X-Second");
        assert_eq!(set_headers[2].name, "X-Third");
    }
}
