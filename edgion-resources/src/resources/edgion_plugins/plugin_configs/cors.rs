//! CORS plugin configuration

use crate::gwapi_types::is_valid_header_token;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;

/// CORS plugin configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CorsConfig {
    /// Allowed origins: "*", "**", "*.example.com" (HTTPS-only shorthand), or comma-separated list.
    /// The `*.domain` shorthand matches `https://<any>.domain` — it does NOT match `http://`.
    /// If http origins must be allowed (e.g. local dev), use `allow_origins_by_regex` instead.
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
    pub origins_cache: Option<HashMap<String, bool>>,

    /// Compiled regex for origin matching
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_origins_regex: Option<Regex>,

    /// Compiled regex for timing origins matching
    #[serde(skip)]
    #[schemars(skip)]
    pub compiled_timing_regex: Option<Regex>,
}

/// Wire value emitted for the `allow_methods: "**"` force-wildcard shorthand.
/// Fixed at compile time so the expansion never allocates on the request path.
const FORCE_WILDCARD_METHODS: &str = "GET,POST,PUT,DELETE,PATCH,HEAD,OPTIONS,CONNECT,TRACE";

/// Wire value emitted for the `allow_headers: "**"` force-wildcard shorthand when
/// the preflight request carries no `Access-Control-Request-Headers` to reflect.
const FORCE_WILDCARD_HEADERS_FALLBACK: &str = "*";

// Security-first defaults: restrictive by default, explicit allow required
fn default_allow_origins() -> String {
    "".to_string()
}

fn default_allow_methods() -> String {
    // Safe read-only methods; POST excluded as it can trigger state changes.
    "GET,HEAD,OPTIONS".to_string()
}

fn default_allow_headers() -> String {
    // CORS-safelisted request headers (always allowed by browsers)
    // Reference: https://fetch.spec.whatwg.org/#cors-safelisted-request-header
    // Range: for single range requests like "bytes=256-"
    "Accept,Accept-Language,Content-Language,Content-Type,Range".to_string()
}

fn default_expose_headers() -> String {
    // Browsers can always access CORS-safe response headers without explicit exposure
    // (Cache-Control, Content-Language, Content-Type, Expires, Last-Modified, Pragma).
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
    fn origin_regex_patterns(&self) -> (Vec<String>, usize) {
        let mut patterns = Vec::new();
        if self.allow_origins.contains(',') {
            for origin in self.allow_origins.split(',') {
                if let Some(domain) = origin.trim().strip_prefix("*.") {
                    patterns.push(format!(r"^https://[^/]+\.{}$", regex::escape(domain)));
                }
            }
        } else if let Some(domain) = self.allow_origins.strip_prefix("*.") {
            patterns.push(format!(r"^https://[^/]+\.{}$", regex::escape(domain)));
        }
        let wildcard_count = patterns.len();
        if let Some(user_patterns) = &self.allow_origins_by_regex {
            patterns.extend(user_patterns.iter().map(|p| format!("^({})$", p)));
        }
        (patterns, wildcard_count)
    }

    fn compile_merged_patterns(
        patterns: &[String],
        field: &str,
        generated_count: usize,
    ) -> Result<Option<Regex>, String> {
        if patterns.is_empty() {
            return Ok(None);
        }
        let merged = patterns
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
        Regex::new(&merged).map(Some).map_err(|_| {
            let failing_index = (generated_count..patterns.len()).find(|&index| {
                let prefix = patterns[..=index]
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
                Regex::new(&prefix).is_err()
            });
            match failing_index {
                Some(index) => format!("Invalid regex in {field} item {}", index - generated_count + 1),
                None => format!("Invalid generated regex in {field}"),
            }
        })
    }

    /// Create a new CorsConfig from deserialized config (for EdgionPlugin system)
    /// This initializes runtime caches after deserialization
    pub fn new(mut config: CorsConfig) -> Result<Self, String> {
        config.validate_and_init()?;
        Ok(config)
    }

    /// Validate configuration and initialize cached values
    pub fn validate_and_init(&mut self) -> Result<(), String> {
        // Keep configuration checks shared with Controller/Gateway ingest.
        if let Some(error) = self.validation_error() {
            return Err(error);
        }

        // Parse comma-separated origins into HashMap.

        if self.allow_origins.contains(',') {
            let mut cache = HashMap::new();
            for origin in self.allow_origins.split(',') {
                let origin = origin.trim();
                if !origin.is_empty() && !origin.starts_with("*.") {
                    cache.insert(origin.to_string(), true);
                }
            }
            if !cache.is_empty() {
                self.origins_cache = Some(cache);
            }
        }

        let (patterns, generated_count) = self.origin_regex_patterns();
        self.compiled_origins_regex =
            Self::compile_merged_patterns(&patterns, "allow_origins_by_regex", generated_count)?;

        // Compile regex patterns for timing origins
        if let Some(ref patterns) = self.timing_allow_origins_by_regex {
            if !patterns.is_empty() {
                let wrapped = patterns.iter().map(|p| format!("({})", p)).collect::<Vec<_>>();
                self.compiled_timing_regex =
                    Self::compile_merged_patterns(&wrapped, "timing_allow_origins_by_regex", 0)?;
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

    /// Get the actual allow-methods value (expand ** if needed).
    ///
    /// Both outcomes are fixed once the configuration is published: the `**`
    /// expansion is a compile-time constant and every other value is the stored
    /// configuration string itself, so this borrows instead of allocating on the
    /// request path.
    pub fn get_allow_methods(&self) -> &str {
        if self.allow_methods == "**" {
            FORCE_WILDCARD_METHODS
        } else {
            &self.allow_methods
        }
    }

    /// Get the actual allow-headers value (may need dynamic reflection).
    ///
    /// Only the `**` shorthand is request-dependent, so it takes ownership of
    /// `requested_headers` and moves it into the result rather than copying it.
    /// Every other configuration borrows the stored string.
    pub fn get_allow_headers(&self, requested_headers: Option<String>) -> Cow<'_, str> {
        if self.allow_headers == "**" {
            // Dynamic reflection: return what client requested
            match requested_headers {
                Some(requested) => Cow::Owned(requested),
                None => Cow::Borrowed(FORCE_WILDCARD_HEADERS_FALLBACK),
            }
        } else {
            Cow::Borrowed(&self.allow_headers)
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

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for CorsConfig {
    fn validation_error(&self) -> Option<String> {
        // Security check: wildcard + credentials conflict
        if self.allow_credentials {
            if self.allow_origins.contains('*') {
                return Some("Cannot use '*' or '**' in allow_origins when allow_credentials is true".to_string());
            }
            if self.allow_methods.contains('*') {
                return Some("Cannot use '*' or '**' in allow_methods when allow_credentials is true".to_string());
            }
            if self.allow_headers.contains('*') {
                return Some("Cannot use '*' or '**' in allow_headers when allow_credentials is true".to_string());
            }
            if self.expose_headers.contains('*') {
                return Some("Cannot use '*' or '**' in expose_headers when allow_credentials is true".to_string());
            }
            if let Some(ref timing) = self.timing_allow_origins {
                if timing.contains('*') {
                    return Some(
                        "Cannot use '*' or '**' in timing_allow_origins when allow_credentials is true".to_string(),
                    );
                }
            }
            // A broad regex (".*", "^https?://.*") bypasses the
            // wildcard guard and reproduces the prohibited ACAO:<origin> + ACAC:true
            // response. Disallow regex allowlists with credentials in both validation paths.
            if self.allow_origins_by_regex.as_ref().is_some_and(|v| !v.is_empty()) {
                return Some(
                    "Cannot use allow_origins_by_regex when allow_credentials is true; \
                     use an explicit comma-separated allow_origins list instead"
                        .to_string(),
                );
            }
            if self
                .timing_allow_origins_by_regex
                .as_ref()
                .is_some_and(|v| !v.is_empty())
            {
                return Some(
                    "Cannot use timing_allow_origins_by_regex when allow_credentials is true; \
                     use an explicit comma-separated timing_allow_origins list instead"
                        .to_string(),
                );
            }
        }

        // Methods and field names share the HTTP token grammar. Check only non-empty
        // list members after SP/HTAB trimming: empty lists/elements, extension methods,
        // case, duplicates, and token punctuation are valid. Do not normalize the wire
        // value or reinterpret the existing exact-match '*' / '**' shorthands.
        for (field, value) in [
            ("allowMethods", self.allow_methods.as_str()),
            ("allowHeaders", self.allow_headers.as_str()),
            ("exposeHeaders", self.expose_headers.as_str()),
        ] {
            for (index, item) in value.split(',').enumerate() {
                let token = item.trim_matches([' ', '\t']);
                if !token.is_empty() && !is_valid_header_token(token) {
                    return Some(format!(
                        "{} list item {} must be an HTTP token; separate names with commas",
                        field,
                        index + 1
                    ));
                }
            }
        }

        let (patterns, generated_count) = self.origin_regex_patterns();
        if let Err(error) = Self::compile_merged_patterns(&patterns, "allow_origins_by_regex", generated_count) {
            return Some(error);
        }
        if let Some(patterns) = &self.timing_allow_origins_by_regex {
            let wrapped = patterns.iter().map(|p| format!("({})", p)).collect::<Vec<_>>();
            if let Err(error) = Self::compile_merged_patterns(&wrapped, "timing_allow_origins_by_regex", 0) {
                return Some(error);
            }
        }

        None
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/cors_test.rs"]
mod tests;
