//! RateLimit plugin configuration
//!
//! Rate limiting using Pingora's Count-Min Sketch (CMS) algorithm.
//! Provides high-performance, memory-efficient rate limiting for high-concurrency scenarios.
//!
//! ## Features:
//! - Count-Min Sketch algorithm with dual-slot sliding window
//! - Fixed memory footprint regardless of key cardinality
//! - Multiple key sources for rate limiting dimension
//! - Rate limit response headers (customizable names)
//!
//! ## Algorithm:
//! Uses Pingora's `Rate` estimator which combines:
//! - Count-Min Sketch for space-efficient frequency estimation
//! - Dual-slot (red/blue) design for sliding window behavior
//! - Lock-free atomic operations for high concurrency

use crate::types::common::KeyGet;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing;

// ========== Rate Limit Scope ==========

/// Rate limit scope: determines how the rate quota is interpreted
///
/// - `Instance`: Each gateway instance enforces the full configured rate independently.
/// - `Cluster`: The configured rate is the total quota for all instances combined.
///   Effective per-instance rate = ceil(rate × skewTolerance / gateway_instance_count).
///
/// ## Example
/// ```yaml
/// rateLimit:
///   rate: 1000
///   scope: Cluster
///   skewTolerance: 1.2
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum RateLimitScope {
    /// Per-instance rate limit (default)
    ///
    /// Each gateway instance enforces the full configured rate independently.
    /// Example: rate=100 means each instance allows 100 req/interval.
    #[default]
    Instance,

    /// Cluster-wide rate limit
    ///
    /// The configured rate is the total quota for all instances combined.
    /// Effective per-instance rate = ceil(rate × skewTolerance / gateway_instance_count).
    /// Example: rate=1000, skewTolerance=1.2, 4 instances → each allows 300 req/interval.
    Cluster,
}

// ========== Public Types (shared with other rate limiting plugins) ==========

/// Custom header names for rate limit responses
///
/// When `headerNames` is not specified, uses default X-RateLimit-* style headers.
/// When `headerNames` is specified, only the configured headers will be shown.
///
/// ## Default behavior (no headerNames configured):
/// - Shows: X-RateLimit-Limit, X-RateLimit-Remaining, X-RateLimit-Reset
///
/// ## Custom headers (only configured ones are shown):
/// ```yaml
/// headerNames:
///   limit: "RateLimit-Limit"      # Only this header will be shown
///   retryIn: "X-Retry-In"         # And this one
///   # remaining and reset are NOT configured, so they won't be shown
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct LimitHeaderNames {
    /// Header name for rate limit value
    /// Only shown if explicitly configured
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<String>,

    /// Header name for remaining quota
    /// Only shown if explicitly configured
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<String>,

    /// Header name for reset timestamp
    /// Only shown if explicitly configured
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset: Option<String>,

    /// Header name for human-readable retry time (e.g., "1.5s", "500ms")
    /// Only shown if explicitly configured
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_in: Option<String>,
}

impl LimitHeaderNames {
    /// Get the limit header name (if configured)
    pub fn limit_header(&self) -> Option<&str> {
        self.limit.as_deref().filter(|s| !s.is_empty())
    }

    /// Get the remaining header name (if configured)
    pub fn remaining_header(&self) -> Option<&str> {
        self.remaining.as_deref().filter(|s| !s.is_empty())
    }

    /// Get the reset header name (if configured)
    pub fn reset_header(&self) -> Option<&str> {
        self.reset.as_deref().filter(|s| !s.is_empty())
    }

    /// Get the retry-in header name (if configured)
    pub fn retry_in_header(&self) -> Option<&str> {
        self.retry_in.as_deref().filter(|s| !s.is_empty())
    }
}

/// Behavior when rate limit key cannot be extracted
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum OnMissingKey {
    /// Allow the request (default, fail-open)
    #[default]
    Allow,
    /// Deny the request
    Deny,
}

// ========== RateLimit Configuration ==========

/// RateLimit plugin configuration
///
/// Uses Pingora's Count-Min Sketch algorithm for rate limiting.
/// This provides memory-efficient rate limiting suitable for high-cardinality keys.
///
/// ## Example:
/// ```yaml
/// rateLimit:
///   rate: 100              # 100 requests per interval
///   interval: "1s"         # 1 second window (default)
///   key:
///     type: clientIp       # Rate limit by IP (default)
///   showLimitHeaders: true
/// ```
///
/// ## Key Configuration Examples:
/// ```yaml
/// # By client IP (default)
/// key:
///   type: clientIp
///
/// # By API key header
/// key:
///   type: header
///   name: "X-API-Key"
///
/// # By client IP + path combination
/// key:
///   type: clientIpAndPath
/// ```
///
/// ## Algorithm Behavior:
/// - `rate.observe()` records events and returns current window count
/// - When count exceeds `rate`, request is rejected
/// - The window slides based on `interval` duration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitConfig {
    /// Maximum requests per interval
    ///
    /// For example, rate=100 with interval="1s" means 100 requests per second.
    pub rate: isize,

    /// Time interval for rate calculation (default: "1s")
    ///
    /// Supports: "1s", "10s", "1m", "5m", etc.
    /// This determines how often the sliding window rotates.
    #[serde(default = "default_interval")]
    pub interval: String,

    /// Rate limiting keys (dimensions)
    ///
    /// Specifies where to extract the rate limiting key from.
    /// Multiple keys are combined with "_" separator.
    ///
    /// ## YAML Examples
    ///
    /// ```yaml
    /// # Single key - limit by client IP
    /// key:
    ///   - type: clientIp
    ///
    /// # Multiple keys - limit by IP + API key + path
    /// key:
    ///   - type: clientIp
    ///   - type: header
    ///     name: "X-Api-Key"
    ///   - type: path
    /// ```
    #[serde(default)]
    pub key: Vec<KeyGet>,

    /// Behavior when key cannot be extracted (default: Allow)
    ///
    /// - `Allow`: Let the request pass without rate limiting (fail-open)
    /// - `Deny`: Reject the request
    ///
    /// Note: If `defaultKey` is configured, it takes precedence over this setting.
    #[serde(default)]
    pub on_missing_key: OnMissingKey,

    /// Default key to use when key cannot be extracted
    ///
    /// When configured, requests without a valid key will use this default key
    /// for rate limiting (all such requests share one rate limit bucket).
    /// This takes precedence over `onMissingKey`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_key: Option<String>,

    /// HTTP status code for rejected requests (default: 429)
    #[serde(default = "default_reject_status")]
    pub reject_status: u16,

    /// Custom rejection message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reject_message: Option<String>,

    /// Show rate limit headers in response (default: true)
    /// Set to false to hide all rate limit headers
    #[serde(default = "default_true")]
    pub show_limit_headers: bool,

    /// Custom header names for rate limit responses
    ///
    /// Allows customizing header names. Defaults to X-RateLimit-* style.
    /// Example for Kong/IETF style:
    /// ```yaml
    /// headerNames:
    ///   limit: "RateLimit-Limit"
    ///   remaining: "RateLimit-Remaining"
    ///   reset: "RateLimit-Reset"
    ///   retryIn: "X-Retry-In"  # optional, shows "1.5s"
    /// ```
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_names: Option<LimitHeaderNames>,

    /// Rate limit scope (default: Instance)
    ///
    /// - `Instance`: rate is per-instance limit (skewTolerance is ignored)
    /// - `Cluster`: rate is the total cluster limit, auto-divided by instance count
    ///
    /// ## Example
    /// ```yaml
    /// rateLimit:
    ///   rate: 1000
    ///   scope: Cluster
    ///   skewTolerance: 1.2
    /// ```
    #[serde(default)]
    pub scope: RateLimitScope,

    /// Skew tolerance for Cluster scope (default: 1.2, range: 1.0 ~ 2.0)
    ///
    /// Compensates for uneven traffic distribution across gateway instances.
    /// Each instance gets `ceil(rate × skewTolerance / instance_count)` as its
    /// effective rate limit.
    ///
    /// - `1.0` = no headroom, strict split (never exceeds configured rate)
    /// - `1.2` = 20% headroom (default, covers typical LB skew ≤60/40)
    /// - `1.5` = 50% headroom (for sticky sessions or large skew)
    /// - `2.0` = 100% headroom (maximum allowed)
    ///
    /// Ignored when scope is Instance.
    #[serde(default = "default_skew_tolerance")]
    pub skew_tolerance: f64,

    /// CMS estimator slots in K units (optional)
    ///
    /// Controls the precision of the Count-Min Sketch algorithm.
    /// Value is in K units (1K = 1024 slots).
    /// If not specified, uses the global `default_estimator_slots_k` from toml config.
    /// Value is capped at global `max_estimator_slots_k`.
    ///
    /// Memory per Rate instance ≈ slots_k × 64KB
    ///
    /// | Value | Slots | Memory |
    /// |-------|-------|--------|
    /// | 1     | 1K    | 64KB   |
    /// | 64    | 64K   | 4MB    |
    /// | 256   | 256K  | 16MB   |
    /// | 1024  | 1M    | 64MB   |
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimator_slots_k: Option<usize>,

    /// Validation error (runtime only)
    #[serde(skip)]
    #[schemars(skip)]
    pub validation_error: Option<String>,

    /// Parsed interval duration (runtime only)
    #[serde(skip)]
    #[schemars(skip)]
    pub interval_duration: Option<Duration>,

    /// Effective estimator slots after validation (runtime only)
    /// This is the actual value used, after applying global defaults and limits.
    #[serde(skip)]
    #[schemars(skip)]
    pub effective_slots: Option<usize>,
}

fn default_interval() -> String {
    "1s".to_string()
}

fn default_reject_status() -> u16 {
    429
}

fn default_true() -> bool {
    true
}

fn default_skew_tolerance() -> f64 {
    1.2
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            rate: 100,
            interval: default_interval(),
            key: vec![KeyGet::default()],
            on_missing_key: OnMissingKey::default(),
            default_key: None,
            reject_status: default_reject_status(),
            reject_message: None,
            show_limit_headers: true,
            header_names: None,
            scope: RateLimitScope::default(),
            skew_tolerance: default_skew_tolerance(),
            estimator_slots_k: None,
            validation_error: None,
            interval_duration: None,
            effective_slots: None,
        }
    }
}

// Re-export from cli config for convenience
pub use crate::core::cli::edgion_gateway::config::{
    get_default_estimator_slots, get_gateway_instance_count, get_max_estimator_slots, get_min_estimator_slots, SLOTS_K,
};

impl RateLimitConfig {
    /// Validate and compile the configuration
    ///
    /// Uses the global RateLimit configuration from toml config file.
    pub fn validate(&mut self) {
        let default_slots = get_default_estimator_slots();
        let max_slots = get_max_estimator_slots();
        self.validate_with_global_config(default_slots, max_slots);
    }

    /// Validate and compile the configuration with global config values
    pub fn validate_with_global_config(&mut self, default_slots: usize, max_slots: usize) {
        if let Err(e) = self.validate_and_compile(default_slots, max_slots) {
            self.validation_error = Some(e);
        }
    }

    /// Check if configuration is valid
    pub fn is_valid(&self) -> bool {
        self.validation_error.is_none()
    }

    /// Get validation error message
    pub fn get_validation_error(&self) -> Option<&str> {
        self.validation_error.as_deref()
    }

    /// Get the parsed interval duration
    pub fn get_interval_duration(&self) -> Duration {
        self.interval_duration.unwrap_or(Duration::from_secs(1))
    }

    /// Get custom header names (if configured)
    pub fn get_header_names(&self) -> Option<&LimitHeaderNames> {
        self.header_names.as_ref()
    }

    /// Get the effective estimator slots
    pub fn get_effective_slots(&self) -> usize {
        self.effective_slots.unwrap_or_else(get_default_estimator_slots)
    }

    /// Get the effective rate for this instance
    ///
    /// In Cluster scope, applies skewTolerance and divides by instance count.
    /// Formula: ceil(rate × skew_tolerance / gateway_count)
    ///
    /// In Instance scope, returns the configured rate as-is.
    pub fn get_effective_rate(&self) -> isize {
        match self.scope {
            RateLimitScope::Instance => self.rate,
            RateLimitScope::Cluster => {
                let count = get_gateway_instance_count() as f64;
                let tolerance = self.skew_tolerance;
                let effective = (self.rate as f64 * tolerance / count).ceil() as isize;
                effective.max(1) // At least 1 request allowed
            }
        }
    }

    fn validate_and_compile(&mut self, default_slots: usize, max_slots: usize) -> Result<(), String> {
        // Validate rate
        if self.rate <= 0 {
            return Err("rate must be greater than 0".to_string());
        }

        // Validate Cluster scope constraints
        if self.scope == RateLimitScope::Cluster {
            // Cluster scope with very low rate may result in 0 effective rate per instance
            if self.rate < 2 {
                return Err("rate must be >= 2 for Cluster scope (to ensure non-zero effective rate)".to_string());
            }
        }

        // Validate and clamp skewTolerance (only meaningful for Cluster, but always validate)
        if self.skew_tolerance < 1.0 {
            tracing::warn!(
                configured = self.skew_tolerance,
                clamped = 1.0,
                "skewTolerance below minimum, clamping to 1.0"
            );
            self.skew_tolerance = 1.0;
        } else if self.skew_tolerance > 2.0 {
            tracing::warn!(
                configured = self.skew_tolerance,
                clamped = 2.0,
                "skewTolerance above maximum, clamping to 2.0"
            );
            self.skew_tolerance = 2.0;
        }

        // Validate reject_status
        if self.reject_status < 100 || self.reject_status >= 600 {
            return Err(format!("Invalid reject_status: {}", self.reject_status));
        }

        // Validate keys: check if name is required but empty for each key
        for (i, key) in self.key.iter().enumerate() {
            if let Some(name) = key.name() {
                if name.is_empty() {
                    return Err(format!(
                        "'key[{}].name' cannot be empty for type {:?}",
                        i,
                        key.source_type()
                    ));
                }
            }

            // Log warning for unsupported KeyGet types in rate limiting
            if matches!(key, KeyGet::Method) {
                tracing::warn!("KeyGet::Method is not recommended for rate limiting (key[{}])", i);
            }
        }

        // Parse interval
        self.interval_duration = Some(parse_duration(&self.interval)?);

        // Compute effective estimator slots
        // Plugin config is in K units, convert to actual slots
        let min_slots = get_min_estimator_slots();
        let configured_slots = self.estimator_slots_k.map(|k| k * SLOTS_K).unwrap_or(default_slots);
        let effective = configured_slots
            .max(min_slots) // At least minimum
            .min(max_slots); // At most maximum

        if configured_slots > max_slots {
            tracing::warn!(
                "estimatorSlotsK {} ({}K) exceeds max {} ({}K), using max value",
                configured_slots,
                configured_slots / SLOTS_K,
                max_slots,
                max_slots / SLOTS_K
            );
        }

        self.effective_slots = Some(effective);

        Ok(())
    }
}

/// Parse duration string (e.g., "500ms", "1s", "2m")
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("Duration string is empty".to_string());
    }

    // Find where the number ends and unit begins
    let (num_str, unit) = s
        .char_indices()
        .find(|(_, c)| !c.is_ascii_digit() && *c != '.')
        .map(|(i, _)| s.split_at(i))
        .unwrap_or((s, "s")); // Default to seconds

    let num: f64 = num_str
        .parse()
        .map_err(|_| format!("Invalid duration number: {}", num_str))?;

    let multiplier = match unit.to_lowercase().as_str() {
        "ms" | "millis" | "milliseconds" => 0.001,
        "s" | "sec" | "seconds" | "" => 1.0,
        "m" | "min" | "minutes" => 60.0,
        "h" | "hour" | "hours" => 3600.0,
        "d" | "day" | "days" => 86400.0,
        _ => return Err(format!("Unknown duration unit: {}", unit)),
    };

    Ok(Duration::from_secs_f64(num * multiplier))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::cli::edgion_gateway::config::set_gateway_instance_count;

    #[test]
    fn test_default_config() {
        let config = RateLimitConfig::default();
        assert_eq!(config.rate, 100);
        assert_eq!(config.interval, "1s");
        assert_eq!(config.reject_status, 429);
        assert!(config.show_limit_headers);
        assert!(config.estimator_slots_k.is_none());
    }

    #[test]
    fn test_validation_rate() {
        let mut config = RateLimitConfig::default();
        config.rate = 0;
        config.validate();
        assert!(!config.is_valid());
        assert!(config.get_validation_error().unwrap().contains("rate"));
    }

    #[test]
    fn test_validation_key_name_empty() {
        let mut config = RateLimitConfig::default();
        config.rate = 10;
        config.key = vec![KeyGet::Header { name: "".to_string() }];
        config.validate();
        assert!(!config.is_valid());
        assert!(config.get_validation_error().unwrap().contains("key[0].name"));
    }

    #[test]
    fn test_validation_success() {
        let mut config = RateLimitConfig::default();
        config.rate = 10;
        config.validate();
        assert!(config.is_valid());
        assert_eq!(config.get_interval_duration(), Duration::from_secs(1));
    }

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_duration("1s").unwrap(), Duration::from_secs(1));
        assert_eq!(parse_duration("2m").unwrap(), Duration::from_secs(120));
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
        assert_eq!(parse_duration("1d").unwrap(), Duration::from_secs(86400));
        assert_eq!(parse_duration("1.5s").unwrap(), Duration::from_millis(1500));
    }

    #[test]
    fn test_interval_parsing() {
        let mut config = RateLimitConfig::default();
        config.rate = 10;
        config.interval = "5s".to_string();
        config.validate();
        assert!(config.is_valid());
        assert_eq!(config.interval_duration, Some(Duration::from_secs(5)));
    }

    #[test]
    fn test_limit_header_names() {
        let headers = LimitHeaderNames {
            limit: Some("X-Limit".to_string()),
            remaining: Some("".to_string()), // Empty string should return None
            reset: None,
            retry_in: Some("Retry-After".to_string()),
        };

        assert_eq!(headers.limit_header(), Some("X-Limit"));
        assert_eq!(headers.remaining_header(), None); // Empty string filtered
        assert_eq!(headers.reset_header(), None);
        assert_eq!(headers.retry_in_header(), Some("Retry-After"));
    }

    #[test]
    fn test_estimator_slots_default() {
        let mut config = RateLimitConfig::default();
        config.rate = 10;
        config.validate();
        assert!(config.is_valid());
        // Default should be 64K = 65536 (from global config)
        assert_eq!(config.get_effective_slots(), get_default_estimator_slots());
    }

    #[test]
    fn test_estimator_slots_custom() {
        let mut config = RateLimitConfig::default();
        config.rate = 10;
        config.estimator_slots_k = Some(16); // 16K = 16384 slots
        config.validate();
        assert!(config.is_valid());
        assert_eq!(config.get_effective_slots(), 16 * SLOTS_K);
    }

    #[test]
    fn test_estimator_slots_capped_at_max() {
        let mut config = RateLimitConfig::default();
        config.rate = 10;
        config.estimator_slots_k = Some(2048); // 2048K, way over max 1024K
                                               // Global config: default=64K, max=256K for this test
        config.validate_with_global_config(64 * SLOTS_K, 256 * SLOTS_K);
        assert!(config.is_valid());
        assert_eq!(config.get_effective_slots(), 256 * SLOTS_K); // Capped at max
    }

    #[test]
    fn test_estimator_slots_minimum() {
        let mut config = RateLimitConfig::default();
        config.rate = 10;
        // Setting to 0 is not valid as a K value, but it will be converted to 0 slots
        // The min check will bump it up
        config.estimator_slots_k = Some(0);
        config.validate();
        assert!(config.is_valid());
        assert_eq!(config.get_effective_slots(), get_min_estimator_slots()); // At least minimum (1K)
    }

    #[test]
    fn test_yaml_key_format() {
        // Test YAML format with array of keys
        let yaml = r#"
rate: 100
interval: "1s"
key:
  - type: header
    name: "X-API-Key"
"#;
        let config: RateLimitConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.rate, 100);
        assert_eq!(
            config.key,
            vec![KeyGet::Header {
                name: "X-API-Key".to_string()
            }]
        );
    }

    #[test]
    fn test_yaml_key_client_ip() {
        // Test clientIp key
        let yaml = r#"
rate: 50
key:
  - type: clientIp
"#;
        let config: RateLimitConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.key, vec![KeyGet::ClientIp]);
    }

    #[test]
    fn test_yaml_key_client_ip_and_path() {
        // Test clientIpAndPath key
        let yaml = r#"
rate: 50
key:
  - type: clientIpAndPath
"#;
        let config: RateLimitConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.key, vec![KeyGet::ClientIpAndPath]);
    }

    #[test]
    fn test_yaml_composite_keys() {
        // Test multiple keys combined
        let yaml = r#"
rate: 100
interval: "1s"
key:
  - type: clientIp
  - type: header
    name: "X-API-Key"
  - type: path
"#;
        let config: RateLimitConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.rate, 100);
        assert_eq!(
            config.key,
            vec![
                KeyGet::ClientIp,
                KeyGet::Header {
                    name: "X-API-Key".to_string()
                },
                KeyGet::Path,
            ]
        );
    }

    #[test]
    fn test_yaml_empty_key_defaults() {
        // Test that empty key array defaults to clientIp
        let yaml = r#"
rate: 50
"#;
        let config: RateLimitConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.key.is_empty()); // serde default is empty vec
    }

    // ==================== Cluster Scope Tests ====================

    #[test]
    fn test_scope_default_is_instance() {
        let config = RateLimitConfig::default();
        assert_eq!(config.scope, RateLimitScope::Instance);
        assert_eq!(config.skew_tolerance, 1.2); // default skewTolerance
    }

    #[test]
    fn test_effective_rate_instance_scope() {
        let config = RateLimitConfig {
            rate: 100,
            ..Default::default()
        };
        // Instance scope: always returns configured rate, skewTolerance ignored
        assert_eq!(config.get_effective_rate(), 100);
    }

    #[test]
    fn test_effective_rate_cluster_scope_with_default_skew() {
        // Simulate 4 gateway instances
        set_gateway_instance_count(4);

        let config = RateLimitConfig {
            rate: 1000,
            scope: RateLimitScope::Cluster,
            skew_tolerance: 1.2,
            ..Default::default()
        };
        // ceil(1000 * 1.2 / 4) = ceil(300.0) = 300
        assert_eq!(config.get_effective_rate(), 300);

        // Cleanup
        set_gateway_instance_count(1);
    }

    #[test]
    fn test_effective_rate_cluster_scope_no_skew() {
        set_gateway_instance_count(4);

        let config = RateLimitConfig {
            rate: 1000,
            scope: RateLimitScope::Cluster,
            skew_tolerance: 1.0,
            ..Default::default()
        };
        // ceil(1000 * 1.0 / 4) = 250
        assert_eq!(config.get_effective_rate(), 250);

        set_gateway_instance_count(1);
    }

    #[test]
    fn test_effective_rate_cluster_scope_high_skew() {
        set_gateway_instance_count(3);

        let config = RateLimitConfig {
            rate: 100,
            scope: RateLimitScope::Cluster,
            skew_tolerance: 1.5,
            ..Default::default()
        };
        // ceil(100 * 1.5 / 3) = ceil(50.0) = 50
        assert_eq!(config.get_effective_rate(), 50);

        set_gateway_instance_count(1);
    }

    #[test]
    fn test_effective_rate_cluster_scope_ceiling() {
        set_gateway_instance_count(3);

        let config = RateLimitConfig {
            rate: 10,
            scope: RateLimitScope::Cluster,
            skew_tolerance: 1.2,
            ..Default::default()
        };
        // ceil(10 * 1.2 / 3) = ceil(4.0) = 4
        assert_eq!(config.get_effective_rate(), 4);

        set_gateway_instance_count(1);
    }

    #[test]
    fn test_effective_rate_cluster_scope_fallback_to_one() {
        // Default count is 1 (or controller unavailable)
        set_gateway_instance_count(1);

        let config = RateLimitConfig {
            rate: 100,
            scope: RateLimitScope::Cluster,
            skew_tolerance: 1.2,
            ..Default::default()
        };
        // ceil(100 * 1.2 / 1) = 120 (skewTolerance still applies)
        assert_eq!(config.get_effective_rate(), 120);
    }

    #[test]
    fn test_effective_rate_cluster_scope_minimum_one() {
        set_gateway_instance_count(100);

        let config = RateLimitConfig {
            rate: 2,
            scope: RateLimitScope::Cluster,
            skew_tolerance: 1.0,
            ..Default::default()
        };
        // ceil(2 * 1.0 / 100) = ceil(0.02) = 1 (clamped to at least 1)
        assert_eq!(config.get_effective_rate(), 1);

        set_gateway_instance_count(1);
    }

    #[test]
    fn test_cluster_scope_validation_low_rate() {
        let mut config = RateLimitConfig {
            rate: 1,
            scope: RateLimitScope::Cluster,
            ..Default::default()
        };
        config.validate();
        assert!(!config.is_valid());
        assert!(config.get_validation_error().unwrap().contains("Cluster scope"));
    }

    #[test]
    fn test_skew_tolerance_clamped_low() {
        let mut config = RateLimitConfig {
            rate: 100,
            scope: RateLimitScope::Cluster,
            skew_tolerance: 0.5, // Below minimum
            ..Default::default()
        };
        config.validate();
        assert!(config.is_valid());
        assert_eq!(config.skew_tolerance, 1.0); // Clamped to 1.0
    }

    #[test]
    fn test_skew_tolerance_clamped_high() {
        let mut config = RateLimitConfig {
            rate: 100,
            scope: RateLimitScope::Cluster,
            skew_tolerance: 3.0, // Above maximum
            ..Default::default()
        };
        config.validate();
        assert!(config.is_valid());
        assert_eq!(config.skew_tolerance, 2.0); // Clamped to 2.0
    }

    #[test]
    fn test_yaml_scope_cluster_with_skew() {
        let yaml = r#"
rate: 1000
interval: "1s"
scope: Cluster
skewTolerance: 1.5
key:
  - type: clientIp
"#;
        let config: RateLimitConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.scope, RateLimitScope::Cluster);
        assert_eq!(config.rate, 1000);
        assert_eq!(config.skew_tolerance, 1.5);
    }

    #[test]
    fn test_yaml_scope_cluster_default_skew() {
        let yaml = r#"
rate: 1000
scope: Cluster
"#;
        let config: RateLimitConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.scope, RateLimitScope::Cluster);
        assert_eq!(config.skew_tolerance, 1.2); // default
    }

    #[test]
    fn test_yaml_scope_default_instance() {
        let yaml = r#"
rate: 100
"#;
        let config: RateLimitConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.scope, RateLimitScope::Instance);
    }

    #[test]
    fn test_effective_rate_dynamic_count_change() {
        // Start with 2 instances
        set_gateway_instance_count(2);

        let config = RateLimitConfig {
            rate: 10,
            scope: RateLimitScope::Cluster,
            skew_tolerance: 1.2,
            ..Default::default()
        };

        // effective_rate = ceil(10 * 1.2 / 2) = 6
        assert_eq!(config.get_effective_rate(), 6);

        // Scale up to 4 instances mid-flight
        set_gateway_instance_count(4);

        // effective_rate = ceil(10 * 1.2 / 4) = ceil(3.0) = 3
        assert_eq!(config.get_effective_rate(), 3);

        // Cleanup
        set_gateway_instance_count(1);
    }
}
