//! CORS plugin configuration

use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// CORS plugin configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CorsConfig {
    /// Allowed origins: "*", "**", "*.example.com", or comma-separated list
    #[serde(default = "default_allow_origins")]
    pub allow_origins: String,

    /// Regex patterns for origin matching
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_origins_by_regex: Option<Vec<String>>,

    /// Allowed HTTP methods: "*", "**", or comma-separated list
    #[serde(default = "default_allow_methods")]
    pub allow_methods: String,

    /// Allowed request headers: "*", "**", or comma-separated list
    #[serde(default = "default_allow_headers")]
    pub allow_headers: String,

    /// Headers exposed to browser: comma-separated list or "*" or "**"
    #[serde(default = "default_expose_headers")]
    pub expose_headers: String,

    /// Whether to allow credentials (cookies, authorization headers)
    #[serde(default)]
    pub allow_credentials: bool,

    /// Preflight cache duration in seconds (None means no Max-Age header)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_age: Option<u32>,

    /// Whether to forward OPTIONS preflight requests to upstream (default: false)
    #[serde(default)]
    pub preflight_continue: bool,

    /// Enable Private Network Access support (Chrome 94+)
    #[serde(default)]
    pub allow_private_network: bool,

    /// Timing-Allow-Origin header value (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing_allow_origins: Option<String>,

    /// Regex patterns for Timing-Allow-Origin matching
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing_allow_origins_by_regex: Option<Vec<String>>,

    // === Cached/computed values ===
    /// Parsed origins as HashMap for O(1) lookup
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) origins_cache: Option<HashMap<String, bool>>,

    /// Compiled regex for origin matching
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) compiled_origins_regex: Option<Regex>,

    /// Compiled regex for timing origins matching
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) compiled_timing_regex: Option<Regex>,
}

// Default functions for serde
// Security-first defaults: restrictive by default, explicit allow required
fn default_allow_origins() -> String {
    // Empty string means no origins allowed - user must explicitly configure
    // This prevents accidental exposure of APIs to all origins
    "".to_string()
}

fn default_allow_methods() -> String {
    // Only safe methods that don't modify data
    // POST is excluded as it can trigger state changes
    "GET,HEAD,OPTIONS".to_string()
}

fn default_allow_headers() -> String {
    // CORS-safelisted request headers (always allowed by browsers)
    // Reference: https://fetch.spec.whatwg.org/#cors-safelisted-request-header
    // Range: for single range requests like "bytes=256-"
    "Accept,Accept-Language,Content-Language,Content-Type,Range".to_string()
}

fn default_expose_headers() -> String {
    // Empty by default - only expose headers explicitly needed
    // Note: Safe response headers are always accessible:
    // Cache-Control, Content-Language, Content-Type, Expires, Last-Modified, Pragma
    "".to_string()
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allow_origins: default_allow_origins(),
            allow_origins_by_regex: None,
            allow_methods: default_allow_methods(),
            allow_headers: default_allow_headers(),
            expose_headers: default_expose_headers(),
            allow_credentials: false,
            max_age: None,
            preflight_continue: false,
            allow_private_network: false,
            timing_allow_origins: None,
            timing_allow_origins_by_regex: None,
            origins_cache: None,
            compiled_origins_regex: None,
            compiled_timing_regex: None,
        }
    }
}

impl CorsConfig {
    /// Create a new CorsConfig from deserialized config (for EdgionPlugin system)
    /// This initializes runtime caches after deserialization
    pub fn new(mut config: CorsConfig) -> Result<Self, String> {
        config.validate_and_init()?;
        Ok(config)
    }

    /// Validate configuration and initialize cached values
    pub fn validate_and_init(&mut self) -> Result<(), String> {
        // Security check: credentials with wildcards
        if self.allow_credentials {
            if self.allow_origins.contains('*') {
                return Err("Cannot use '*' or '**' in allow_origins when allow_credentials is true".to_string());
            }
            if self.allow_methods.contains('*') {
                return Err("Cannot use '*' or '**' in allow_methods when allow_credentials is true".to_string());
            }
            if self.allow_headers.contains('*') {
                return Err("Cannot use '*' or '**' in allow_headers when allow_credentials is true".to_string());
            }
            if self.expose_headers.contains('*') {
                return Err("Cannot use '*' or '**' in expose_headers when allow_credentials is true".to_string());
            }
            if let Some(ref timing) = self.timing_allow_origins {
                if timing.contains('*') {
                    return Err(
                        "Cannot use '*' or '**' in timing_allow_origins when allow_credentials is true".to_string(),
                    );
                }
            }
        }

        // Parse comma-separated origins into HashMap and collect wildcard patterns
        let mut wildcard_patterns = Vec::new();

        if self.allow_origins.contains(',') {
            let mut cache = HashMap::new();
            for origin in self.allow_origins.split(',') {
                let origin = origin.trim();
                if !origin.is_empty() {
                    // Check for subdomain wildcard: *.example.com
                    if let Some(domain) = origin.strip_prefix("*.") {
                        // Convert to regex: ^https?://[^/]+\.example\.com$
                        // Remove "*."
                        let pattern = format!(r"^https?://[^/]+\.{}$", regex::escape(domain));
                        wildcard_patterns.push(pattern);
                    } else {
                        cache.insert(origin.to_string(), true);
                    }
                }
            }
            if !cache.is_empty() {
                self.origins_cache = Some(cache);
            }
        } else if self.allow_origins.starts_with("*.") {
            // Single subdomain wildcard
            let domain = &self.allow_origins[2..];
            let pattern = format!(r"^https?://[^/]+\.{}$", regex::escape(domain));
            wildcard_patterns.push(pattern);
        }

        // Compile regex patterns for origins (including wildcards and user-provided regex)
        let mut all_patterns = wildcard_patterns;

        if let Some(ref patterns) = self.allow_origins_by_regex {
            all_patterns.extend(patterns.iter().map(|p| format!("({})", p)));
        }

        if !all_patterns.is_empty() {
            // Merge all patterns into one regex: (pattern1)|(pattern2)|...
            let merged = all_patterns
                .iter()
                .map(|p| {
                    if p.starts_with('(') {
                        p.clone()
                    } else {
                        format!("({})", p)
                    }
                })
                .collect::<Vec<_>>()
                .join("|");

            self.compiled_origins_regex =
                Some(Regex::new(&merged).map_err(|e| format!("Invalid regex pattern: {}", e))?);
        }

        // Compile regex patterns for timing origins
        if let Some(ref patterns) = self.timing_allow_origins_by_regex {
            if !patterns.is_empty() {
                let merged = patterns
                    .iter()
                    .map(|p| format!("({})", p))
                    .collect::<Vec<_>>()
                    .join("|");

                self.compiled_timing_regex = Some(
                    Regex::new(&merged)
                        .map_err(|e| format!("Invalid regex in timing_allow_origins_by_regex: {}", e))?,
                );
            }
        }

        Ok(())
    }

    /// Check if an origin is allowed
    pub fn is_origin_allowed(&self, origin: &str) -> bool {
        // Reject null origin
        if origin == "null" {
            return false;
        }

        // Check wildcard
        if self.allow_origins == "*" || self.allow_origins == "**" {
            return true;
        }

        // Check exact match in cache (comma-separated list)
        if let Some(ref cache) = self.origins_cache {
            if cache.contains_key(origin) {
                return true;
            }
        } else {
            // Single origin exact match
            if self.allow_origins == origin {
                return true;
            }
        }

        // Check regex match
        if let Some(ref regex) = self.compiled_origins_regex {
            if regex.is_match(origin) {
                return true;
            }
        }

        false
    }

    /// Check if an origin is allowed for timing information
    pub fn is_timing_origin_allowed(&self, origin: &str) -> bool {
        // No timing configuration
        if self.timing_allow_origins.is_none() && self.timing_allow_origins_by_regex.is_none() {
            return false;
        }

        // Reject null origin
        if origin == "null" {
            return false;
        }

        // Check wildcard
        if let Some(ref timing_origins) = self.timing_allow_origins {
            if timing_origins == "*" || timing_origins == "**" {
                return true;
            }
            if timing_origins == origin {
                return true;
            }
        }

        // Check regex match
        if let Some(ref regex) = self.compiled_timing_regex {
            if regex.is_match(origin) {
                return true;
            }
        }

        false
    }

    /// Get the actual allow-methods value (expand ** if needed)
    pub fn get_allow_methods(&self) -> String {
        if self.allow_methods == "**" {
            "GET,POST,PUT,DELETE,PATCH,HEAD,OPTIONS,CONNECT,TRACE".to_string()
        } else {
            self.allow_methods.clone()
        }
    }

    /// Get the actual allow-headers value (may need dynamic reflection)
    pub fn get_allow_headers(&self, requested_headers: Option<&str>) -> String {
        if self.allow_headers == "**" {
            // Dynamic reflection: return what client requested
            requested_headers.unwrap_or("*").to_string()
        } else {
            self.allow_headers.clone()
        }
    }

    /// Get the actual expose-headers value
    pub fn get_expose_headers(&self) -> &str {
        &self.expose_headers
    }

    /// Should add Vary: Origin header?
    pub fn should_add_vary_header(&self) -> bool {
        self.allow_origins != "*"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cors = CorsConfig::default();

        // Security-first defaults
        assert_eq!(cors.allow_origins, ""); // No origins allowed by default
        assert_eq!(cors.allow_methods, "GET,HEAD,OPTIONS"); // Only safe methods
        assert_eq!(
            cors.allow_headers,
            "Accept,Accept-Language,Content-Language,Content-Type,Range"
        );
        assert_eq!(cors.expose_headers, ""); // No extra headers exposed
        assert!(!cors.allow_credentials);
        assert_eq!(cors.max_age, None);
        assert!(!cors.preflight_continue);
        assert!(!cors.allow_private_network);
    }

    #[test]
    fn test_default_config_rejects_all_origins() {
        let mut cors = CorsConfig::default();
        cors.validate_and_init().unwrap();

        // Default empty origin should reject all requests
        assert!(!cors.is_origin_allowed("https://example.com"));
        assert!(!cors.is_origin_allowed("https://trusted.com"));
        assert!(!cors.is_origin_allowed("http://localhost:3000"));
    }

    #[test]
    fn test_credentials_with_wildcard_fails() {
        let mut config = CorsConfig {
            allow_origins: "*".to_string(),
            allow_credentials: true,
            ..Default::default()
        };

        let result = config.validate_and_init();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("allow_credentials"));
    }

    #[test]
    fn test_comma_separated_origins() {
        let mut cors = CorsConfig {
            allow_origins: "https://a.com,https://b.com,https://c.com".to_string(),
            allow_methods: "GET,POST".to_string(),
            ..Default::default()
        };
        cors.validate_and_init().unwrap();

        assert!(cors.is_origin_allowed("https://a.com"));
        assert!(cors.is_origin_allowed("https://b.com"));
        assert!(cors.is_origin_allowed("https://c.com"));
        assert!(!cors.is_origin_allowed("https://d.com"));
    }

    #[test]
    fn test_null_origin_rejected() {
        let mut cors = CorsConfig {
            allow_origins: "*".to_string(),
            ..Default::default()
        };
        cors.validate_and_init().unwrap();

        assert!(!cors.is_origin_allowed("null"));
    }

    #[test]
    fn test_regex_matching() {
        let mut cors = CorsConfig {
            allow_origins: "https://example.com".to_string(),
            allow_origins_by_regex: Some(vec![
                "^https://.*\\.test\\.com$".to_string(),
                "^http://localhost:[0-9]+$".to_string(),
            ]),
            allow_methods: "GET,POST".to_string(),
            ..Default::default()
        };
        cors.validate_and_init().unwrap();

        assert!(cors.is_origin_allowed("https://a.test.com"));
        assert!(cors.is_origin_allowed("https://b.test.com"));
        assert!(cors.is_origin_allowed("http://localhost:3000"));
        assert!(cors.is_origin_allowed("http://localhost:8080"));
        assert!(cors.is_origin_allowed("https://example.com"));
        assert!(!cors.is_origin_allowed("https://test.com"));
        assert!(!cors.is_origin_allowed("http://localhost"));
    }

    #[test]
    fn test_force_wildcard() {
        let mut cors = CorsConfig {
            allow_origins: "**".to_string(),
            ..Default::default()
        };
        cors.validate_and_init().unwrap();

        assert!(cors.is_origin_allowed("https://any.com"));
        assert!(cors.is_origin_allowed("http://localhost:3000"));
    }

    #[test]
    fn test_methods_expansion() {
        let cors = CorsConfig {
            allow_origins: "https://example.com".to_string(),
            allow_methods: "**".to_string(),
            ..Default::default()
        };
        let methods = cors.get_allow_methods();

        assert!(methods.contains("GET"));
        assert!(methods.contains("POST"));
        assert!(methods.contains("PUT"));
        assert!(methods.contains("DELETE"));
        assert!(methods.contains("PATCH"));
    }

    #[test]
    fn test_headers_reflection() {
        let cors = CorsConfig {
            allow_origins: "https://example.com".to_string(),
            allow_headers: "**".to_string(),
            ..Default::default()
        };
        let headers = cors.get_allow_headers(Some("Content-Type,X-Custom"));

        assert_eq!(headers, "Content-Type,X-Custom");
    }

    #[test]
    fn test_timing_origin_matching() {
        let mut cors = CorsConfig {
            allow_origins: "https://example.com".to_string(),
            timing_allow_origins: Some("https://analytics.com".to_string()),
            timing_allow_origins_by_regex: Some(vec!["^https://.*\\.cdn\\.com$".to_string()]),
            ..Default::default()
        };
        cors.validate_and_init().unwrap();

        assert!(cors.is_timing_origin_allowed("https://analytics.com"));
        assert!(cors.is_timing_origin_allowed("https://a.cdn.com"));
        assert!(!cors.is_timing_origin_allowed("https://other.com"));
    }

    #[test]
    fn test_vary_header_logic() {
        let cors1 = CorsConfig {
            allow_origins: "*".to_string(),
            ..Default::default()
        };
        assert!(!cors1.should_add_vary_header());

        let cors2 = CorsConfig {
            allow_origins: "https://example.com".to_string(),
            ..Default::default()
        };
        assert!(cors2.should_add_vary_header());
    }

    #[test]
    fn test_subdomain_wildcard() {
        let mut cors = CorsConfig {
            allow_origins: "*.example.com".to_string(),
            allow_methods: "GET,POST".to_string(),
            ..Default::default()
        };
        cors.validate_and_init().unwrap();

        assert!(cors.is_origin_allowed("https://a.example.com"));
        assert!(cors.is_origin_allowed("https://b.example.com"));
        assert!(cors.is_origin_allowed("http://test.example.com"));
        assert!(!cors.is_origin_allowed("https://example.com")); // Root domain not included
        assert!(!cors.is_origin_allowed("https://other.com"));
    }

    #[test]
    fn test_comma_separated_with_wildcard() {
        let mut cors = CorsConfig {
            allow_origins: "https://exact.com,*.example.com,https://another.com".to_string(),
            allow_methods: "GET,POST".to_string(),
            ..Default::default()
        };
        cors.validate_and_init().unwrap();

        assert!(cors.is_origin_allowed("https://exact.com"));
        assert!(cors.is_origin_allowed("https://sub.example.com"));
        assert!(cors.is_origin_allowed("https://another.com"));
        assert!(!cors.is_origin_allowed("https://example.com"));
    }

    #[test]
    fn test_preflight_continue_config() {
        let cors1 = CorsConfig {
            allow_origins: "https://example.com".to_string(),
            preflight_continue: true,
            ..Default::default()
        };
        assert!(cors1.preflight_continue);

        let cors2 = CorsConfig {
            allow_origins: "https://example.com".to_string(),
            preflight_continue: false,
            ..Default::default()
        };
        assert!(!cors2.preflight_continue);

        let cors3 = CorsConfig::default();
        assert!(!cors3.preflight_continue);
    }

    #[test]
    fn test_private_network_config() {
        let cors1 = CorsConfig {
            allow_private_network: true,
            ..Default::default()
        };
        assert!(cors1.allow_private_network);

        let cors2 = CorsConfig::default();
        assert!(!cors2.allow_private_network);
    }
}
