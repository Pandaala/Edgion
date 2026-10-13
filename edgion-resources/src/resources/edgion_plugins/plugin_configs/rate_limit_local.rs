//! RateLimitLocal plugin configuration
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

use crate::common::KeyGet;
use crate::duration::parse_duration;
use crate::response_header_policy::is_protected_response_header;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashSet;
use std::time::Duration;
use tracing;

fn deserialize_non_empty_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    Ok(opt.filter(|s| !s.is_empty()))
}

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
pub enum RateLimitLocalScope {
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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_non_empty_string"
    )]
    pub limit: Option<String>,

    /// Header name for remaining quota
    /// Only shown if explicitly configured
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_non_empty_string"
    )]
    pub remaining: Option<String>,

    /// Header name for reset timestamp
    /// Only shown if explicitly configured.
    ///
    /// Note: behavior differs by plugin. `RateLimitLocal` (in-process CMS) emits this
    /// on both allowed and rejected responses. `RateLimitRedis` only emits it on
    /// rejection (429) responses — the Redis Lua scripts do not return a
    /// window-reset timestamp for allowed requests.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_non_empty_string"
    )]
    pub reset: Option<String>,

    /// Header name for human-readable retry time (e.g., "1.5s", "500ms")
    /// Only shown if explicitly configured
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_non_empty_string"
    )]
    pub retry_in: Option<String>,
}

impl LimitHeaderNames {
    /// Validate configured response header names at the shared resource boundary.
    pub fn validation_error(&self) -> Option<String> {
        let mut normalized_names = HashSet::with_capacity(4);
        for (field, name) in [
            ("limit", self.limit_header()),
            ("remaining", self.remaining_header()),
            ("reset", self.reset_header()),
            ("retryIn", self.retry_in_header()),
        ] {
            let Some(name) = name else {
                continue;
            };
            if name.len() > crate::gwapi_types::HEADER_NAME_MAX_LEN as usize
                || !crate::gwapi_types::is_valid_header_token(name)
            {
                return Some(format!(
                    "headerNames.{field} must be a valid HTTP header name of at most 256 bytes"
                ));
            }
            if is_protected_response_header(name) {
                return Some(format!(
                    "headerNames.{field} must not use an HTTP framing or hop-by-hop header name"
                ));
            }
            if !normalized_names.insert(name.to_ascii_lowercase()) {
                return Some("headerNames must not contain case-insensitive duplicate names".to_string());
            }
        }
        None
    }

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
pub enum OnKeyMiss {
    /// Allow the request (default, fail-open)
    #[default]
    Allow,
    /// Deny the request
    Deny,
}

// ========== RateLimitLocal Configuration ==========

/// Action after the local allowance is exhausted.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
pub enum OnLimitExceeded {
    /// Reject immediately, preserving ordinary local rate limiting.
    #[default]
    Reject,
    /// Continue so a later conditional Redis limiter can supply overflow allowance.
    Continue,
}

/// RateLimitLocal plugin configuration
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
///     type: remoteIp       # Rate limit by resolved IP (default)
///   showLimitHeaders: true
/// ```
///
/// ## Key Configuration Examples:
/// ```yaml
/// # By client IP (default)
/// key:
///   type: remoteIp
///
/// # By API key header
/// key:
///   type: header
///   name: "X-API-Key"
///
/// # By client IP + path combination
/// key:
///   type: remoteIpAndPath
/// ```
///
/// ## Algorithm Behavior:
/// - `rate.observe()` records events and returns current window count
/// - When count exceeds `rate`, request is rejected
/// - The window slides based on `interval` duration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitLocalConfig {
    /// Maximum requests per interval
    ///
    /// For example, rate=100 with interval="1s" means 100 requests per second.
    #[schemars(range(min = 1))]
    pub rate: isize,

    /// Time interval for rate calculation (default: "1s")
    ///
    /// Supports GEP-2257 durations: "1s", "10s", "1m", "5m", etc.
    /// This determines how often the sliding window rotates.
    #[serde(default = "default_interval")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub interval: String,

    /// Rate limiting keys (dimensions)
    ///
    /// Specifies where to extract the rate limiting key from.
    /// A single key is used verbatim; multiple keys are combined into an
    /// unambiguous composite key (`<byte-len>:<value>` per configured key,
    /// "-" when it does not resolve, joined with "_").
    ///
    /// Default: [{ type: remoteIp }]. An explicitly empty list is a valid
    /// configuration meaning "one global bucket": it is not a key miss, so it
    /// still rate-limits rather than falling through to `onKeyMiss`.
    ///
    /// ## YAML Examples
    ///
    /// ```yaml
    /// # Single key - limit by client IP
    /// key:
    ///   - type: remoteIp
    ///
    /// # Multiple keys - limit by IP + API key + path
    /// key:
    ///   - type: remoteIp
    ///   - type: header
    ///     name: "X-Api-Key"
    ///   - type: path
    /// ```
    #[serde(default = "default_key_sources")]
    pub key: Vec<KeyGet>,

    /// Behavior when key cannot be extracted (default: Allow)
    ///
    /// - `Allow`: Let the request pass without rate limiting (fail-open)
    /// - `Deny`: Reject the request
    ///
    /// Note: If `defaultKey` is configured, it takes precedence over this setting.
    #[serde(default)]
    pub on_key_miss: OnKeyMiss,

    /// Action when the local limit is exceeded (default: Reject).
    #[serde(default)]
    pub on_limit_exceeded: OnLimitExceeded,

    /// Default key to use when key cannot be extracted
    ///
    /// When configured, requests without a valid key will use this default key
    /// for rate limiting (all such requests share one rate limit bucket).
    /// This takes precedence over `onKeyMiss`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_key: Option<String>,

    /// HTTP status code for rejected requests (default: 429)
    #[serde(default = "default_reject_status")]
    #[schemars(range(min = 200, max = 599))]
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
    pub scope: RateLimitLocalScope,

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
    /// If not specified, uses the global `default_estimator_slots_k` from YAML config.
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

/// Default key sources: rate-limit per client IP.
///
/// Must match the `Default` impl and the documented default — `#[serde(default)]`
/// would yield an empty list instead, which is a different (global-bucket) policy.
fn default_key_sources() -> Vec<KeyGet> {
    vec![KeyGet::RemoteIp]
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

impl Default for RateLimitLocalConfig {
    fn default() -> Self {
        Self {
            rate: 100,
            interval: default_interval(),
            key: vec![KeyGet::default()],
            on_key_miss: OnKeyMiss::default(),
            on_limit_exceeded: OnLimitExceeded::default(),
            default_key: None,
            reject_status: default_reject_status(),
            reject_message: None,
            show_limit_headers: true,
            header_names: None,
            scope: RateLimitLocalScope::default(),
            skew_tolerance: default_skew_tolerance(),
            estimator_slots_k: None,
            interval_duration: None,
            effective_slots: None,
        }
    }
}

// Re-export from cli config for convenience
pub use crate::rate_limit_local_config::{
    get_default_estimator_slots, get_gateway_instance_count, get_max_estimator_slots, get_min_estimator_slots, SLOTS_K,
};

impl RateLimitLocalConfig {
    /// Validate and compile the configuration.
    ///
    /// Uses the global RateLimitLocal configuration from YAML config file.
    /// Returns `Err(message)` if invalid; on success, `interval_duration` and
    /// `effective_slots` are populated.
    pub fn validate(&mut self) -> Result<(), String> {
        let default_slots = get_default_estimator_slots();
        let max_slots = get_max_estimator_slots();
        self.validate_with_global_config(default_slots, max_slots)
    }

    /// Validate and compile the configuration with explicit global config values.
    pub fn validate_with_global_config(&mut self, default_slots: usize, max_slots: usize) -> Result<(), String> {
        self.validate_and_compile(default_slots, max_slots)
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
        self.effective_rate_for_instances(get_gateway_instance_count())
    }

    /// Compute the effective rate for a given number of gateway instances.
    ///
    /// This is the pure-computation core extracted for testability
    /// (avoids global state dependency in unit tests).
    pub fn effective_rate_for_instances(&self, gateway_count: u32) -> isize {
        match self.scope {
            RateLimitLocalScope::Instance => self.rate,
            RateLimitLocalScope::Cluster => {
                let count = (gateway_count.max(1)) as f64;
                let tolerance = self.skew_tolerance;
                let effective = (self.rate as f64 * tolerance / count).ceil() as isize;
                effective.max(1) // At least 1 request allowed
            }
        }
    }

    /// Check fields for validation errors without mutating any state.
    ///
    /// Only returns errors for structurally invalid configurations.
    /// Clamp/assignment side effects (skew_tolerance, interval_duration, effective_slots)
    /// are intentionally NOT performed here — they remain in `validate_and_compile`.
    fn check_fields(&self) -> Option<String> {
        // Validate rate
        if self.rate <= 0 {
            return Some("rate must be greater than 0".to_string());
        }

        // Validate Cluster scope constraints
        if self.scope == RateLimitLocalScope::Cluster && self.rate < 2 {
            return Some("rate must be >= 2 for Cluster scope (to ensure non-zero effective rate)".to_string());
        }

        // Validate reject_status
        if !crate::http_status::is_valid_final_http_status(self.reject_status) {
            return Some(format!("Invalid reject_status: {}", self.reject_status));
        }

        if let Some(error) = self.header_names.as_ref().and_then(LimitHeaderNames::validation_error) {
            return Some(error);
        }

        // Validate keys: check if name is required but empty for each key
        for (i, key) in self.key.iter().enumerate() {
            if let Some(name) = key.name() {
                if name.is_empty() {
                    return Some(format!(
                        "'key[{}].name' cannot be empty for type {:?}",
                        i,
                        key.source_type()
                    ));
                }
            }
        }

        // Validate interval can be parsed (do not store the result)
        if let Err(e) = parse_duration(&self.interval) {
            return Some(e.to_string());
        }

        None
    }

    fn validate_and_compile(&mut self, default_slots: usize, max_slots: usize) -> Result<(), String> {
        if let Some(err) = self.check_fields() {
            return Err(err);
        }

        // Clamp skewTolerance (only meaningful for Cluster, but always apply)
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

        // Parse interval and store
        self.interval_duration = Some(parse_duration(&self.interval).map_err(|e| e.to_string())?);

        // Compute effective estimator slots
        // Plugin config is in K units, convert to actual slots
        let min_slots = get_min_estimator_slots();
        let configured_slots = self
            .estimator_slots_k
            .map(|k| k.saturating_mul(SLOTS_K))
            .unwrap_or(default_slots);
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

        // Log warning for unsupported KeyGet types in rate limiting
        for (i, key) in self.key.iter().enumerate() {
            if matches!(key, KeyGet::Method) {
                tracing::warn!("KeyGet::Method is not recommended for rate limiting (key[{}])", i);
            }
        }

        Ok(())
    }
}

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for RateLimitLocalConfig {
    fn validation_error(&self) -> Option<String> {
        self.check_fields()
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/rate_limit_local_test.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/rate_limit_local_test/validation_trait_tests.rs"]
mod validation_trait_tests;
