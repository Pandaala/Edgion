//! RateLimitRedis plugin configuration
//!
//! Redis-based precise rate limiting for cluster-wide enforcement.
//! Supports multiple rate policies (e.g., 10/s AND 1000/h),
//! multiple algorithms (sliding window, fixed window, token bucket),
//! and true cluster-wide enforcement via Redis Lua scripts.
//!
//! ## Architecture:
//! - All rate limit checks are atomic via Redis Lua scripts (EVAL/EVALSHA)
//! - Single KEYS design for Redis Cluster compatibility (no CROSSSLOT)
//! - EVALSHA with automatic EVAL fallback on NOSCRIPT
//! - Process-shared immutable Lua source/SHA descriptors; EVAL fallback only on an exact NOSCRIPT error
//!
//! ## Complementary to RateLimitLocal:
//! - RateLimitLocal: in-process CMS, ~nanosecond, approximate, per-instance
//! - RateLimitRedis: Redis-backed, ~0.1-1ms, precise, cluster-wide

use crate::common::KeyGet;
use crate::duration::parse_duration;
use crate::redis_rate_limit::{validate_rate_limit_units, validate_rate_limit_window};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::rate_limit_local::{LimitHeaderNames, OnKeyMiss};

// ========== Rate Limit Algorithm ==========

/// Rate limiting algorithm selection.
///
/// Each algorithm trades off between accuracy, burst behavior, and Redis cost.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
pub enum RateLimitAlgorithm {
    /// Sliding window counter (default) — best accuracy/performance balance.
    ///
    /// Uses weighted average of current + previous fixed window.
    /// Prevents boundary burst.
    /// Redis: 1 HASH key per policy (stores cur, prev, cur_ts fields).
    #[default]
    SlidingWindow,

    /// Fixed window — simplest, but has boundary burst issue.
    ///
    /// Resets counter at each window boundary.
    /// Redis: 1 STRING key per policy per window.
    FixedWindow,

    /// Token bucket — allows controlled bursts up to bucket capacity.
    ///
    /// `rate` = refill rate per interval. Bucket capacity = rate.
    /// Redis: 1 HASH key per policy (tokens + timestamp fields).
    TokenBucket,
}

impl RateLimitAlgorithm {
    /// Short identifier for Redis key prefix.
    pub fn key_prefix(&self) -> &'static str {
        match self {
            RateLimitAlgorithm::SlidingWindow => "sw",
            RateLimitAlgorithm::FixedWindow => "fw",
            RateLimitAlgorithm::TokenBucket => "tb",
        }
    }
}

// ========== On Redis Failure ==========

/// Behavior when Redis is unavailable (connection error, timeout, etc.)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum OnRedisFailure {
    /// Allow the request through (fail-open, default).
    ///
    /// Recommended for most scenarios. Rate limiting degrades gracefully
    /// when Redis is temporarily unavailable.
    #[default]
    Allow,

    /// Deny the request (fail-close).
    ///
    /// Use for high-security scenarios where exceeding limits is unacceptable.
    /// Warning: Redis outage will block ALL requests.
    Deny,
}

// ========== Rate Limit Policy ==========

/// A single rate limit policy with its own window and algorithm.
///
/// Multiple policies can be combined for layered limiting,
/// e.g., a per-second burst limit AND a per-hour quota.
///
/// ## Example
/// ```yaml
/// policies:
///   - rate: 10
///     interval: "1s"
///   - rate: 1000
///     interval: "1h"
///     algorithm: FixedWindow
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitPolicy {
    /// Maximum requests (or cost units) allowed per interval.
    #[schemars(range(min = 1, max = 1000000))]
    pub rate: u64,

    /// Time interval (GEP-2257 duration, e.g., "1s", "1m", "1h", "24h").
    /// Uses the same parse_duration() as RateLimitLocal.
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub interval: String,

    /// Rate limiting algorithm (default: SlidingWindow).
    #[serde(default)]
    pub algorithm: RateLimitAlgorithm,

    // --- Runtime fields ---
    /// Parsed interval duration (set during validation)
    #[serde(skip)]
    #[schemars(skip)]
    pub interval_duration: Option<Duration>,
}

// ========== RateLimitRedis Configuration ==========

/// Redis-based precise rate limiting plugin configuration.
///
/// Supports multiple rate policies (e.g., 10/s AND 1000/h),
/// multiple algorithms, and true cluster-wide enforcement via Redis.
///
/// ## Example
/// ```yaml
/// type: RateLimitRedis
/// config:
///   redisRef: "default/shared-redis"
///   key:
///     - type: remoteIp
///   policies:
///     - rate: 100
///       interval: "1s"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitRedisConfig {
    /// Reference to Redis LinkSys resource ("namespace/name").
    ///
    /// Must point to an existing LinkSys of type Redis.
    /// The plugin resolves the client at runtime via get_redis_client().
    pub redis_ref: String,

    /// Rate limiting policies (at least one required).
    ///
    /// Multiple policies are evaluated independently — ALL must pass.
    /// The most restrictive policy determines the rejection.
    ///
    /// ## Example
    /// ```yaml
    /// policies:
    ///   - rate: 10
    ///     interval: "1s"
    ///   - rate: 1000
    ///     interval: "1h"
    /// ```
    pub policies: Vec<RateLimitPolicy>,

    /// Key extraction sources (same KeyGet as RateLimitLocal).
    ///
    /// A single key is used verbatim; multiple keys are combined into an
    /// unambiguous composite key (`<byte-len>:<value>` per configured key,
    /// "-" when it does not resolve, joined with "_").
    /// Default: [{ type: remoteIp }]. An explicitly empty list is a valid
    /// configuration meaning "one global bucket": it is not a key miss, so it
    /// still rate-limits rather than falling through to `onKeyMiss`.
    #[serde(default = "default_key_sources")]
    pub key: Vec<KeyGet>,

    /// Behavior when key cannot be extracted (default: Allow)
    #[serde(default)]
    pub on_key_miss: OnKeyMiss,

    /// Default key fallback when extraction fails
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_key: Option<String>,

    /// HTTP status code for rejected requests (default: 429)
    #[serde(default = "default_reject_status")]
    #[schemars(range(min = 200, max = 599))]
    pub reject_status: u16,

    /// Custom rejection message (JSON body)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reject_message: Option<String>,

    /// Show rate limit headers in response (default: true)
    #[serde(default = "default_true")]
    pub show_limit_headers: bool,

    /// Custom header names for rate limit responses.
    /// Reuses LimitHeaderNames from RateLimitLocal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_names: Option<LimitHeaderNames>,

    /// Redis key prefix (default: "edgion:rl:")
    #[serde(default = "default_key_prefix")]
    pub key_prefix: String,

    /// Behavior when Redis is unavailable (default: Allow)
    #[serde(default)]
    pub on_redis_failure: OnRedisFailure,

    /// Cost per request for weighted rate limiting (default: 1).
    ///
    /// Useful for APIs where different endpoints have different costs.
    /// For example, a search endpoint might cost 5, while a simple GET costs 1.
    #[serde(default = "default_cost")]
    #[schemars(range(min = 1, max = 1000000))]
    pub cost: u32,

    /// Maximum length of the limit key portion in Redis keys (default: 4096 bytes).
    ///
    /// Keys extracted from requests (IP, Header, Cookie, etc.) are truncated
    /// to this length before being embedded into the Redis key string.
    /// This prevents Redis OOM from maliciously long header values.
    #[serde(default = "default_max_key_len")]
    #[schemars(range(min = 64, max = 65536))]
    pub max_key_len: usize,

    /// Optional aggregate timeout for the whole Redis check (duration string,
    /// e.g. "500ms"). Default unset = no wall-clock bound (the per-command
    /// timeout from the LinkSys Redis CRD still applies). When set, caps the
    /// total latency of a multi-policy check; on elapse the configured
    /// `on_redis_failure` policy applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub request_timeout: Option<String>,

    /// Parsed `request_timeout` (runtime only).
    #[serde(skip)]
    #[schemars(skip)]
    pub request_timeout_duration: Option<Duration>,
}

// ========== Default Functions ==========

fn default_reject_status() -> u16 {
    429
}

fn default_true() -> bool {
    true
}

/// Default key sources: rate-limit per client IP.
///
/// Must match the `Default` impl and the documented default — `#[serde(default)]`
/// would yield an empty list instead, which is a different (global-bucket) policy.
fn default_key_sources() -> Vec<KeyGet> {
    vec![KeyGet::RemoteIp]
}

fn default_key_prefix() -> String {
    "edgion:rl:".to_string()
}

fn default_cost() -> u32 {
    1
}

fn default_max_key_len() -> usize {
    4096
}

// ========== Default Implementation ==========

impl Default for RateLimitRedisConfig {
    fn default() -> Self {
        Self {
            redis_ref: String::new(),
            policies: vec![],
            key: vec![KeyGet::default()],
            on_key_miss: OnKeyMiss::default(),
            default_key: None,
            reject_status: default_reject_status(),
            reject_message: None,
            show_limit_headers: true,
            header_names: None,
            key_prefix: default_key_prefix(),
            on_redis_failure: OnRedisFailure::default(),
            cost: default_cost(),
            max_key_len: default_max_key_len(),
            request_timeout: None,
            request_timeout_duration: None,
        }
    }
}

// ========== Validation ==========

impl RateLimitRedisConfig {
    /// Validate the configuration and populate `interval_duration` for each policy.
    ///
    /// Returns `Err(message)` if any field is invalid. On success, every policy
    /// has its `interval_duration` parsed and stored.
    pub fn validate(&mut self) -> Result<(), String> {
        self.validate_inner()
    }

    /// Get custom header names (if configured).
    pub fn get_header_names(&self) -> Option<&LimitHeaderNames> {
        self.header_names.as_ref()
    }

    fn check_fields(&self) -> Option<String> {
        // Mirror every return Err(...) branch from validate_inner,
        // but DO NOT mirror the `policy.interval_duration = Some(dur)` assignment.

        // redis_ref format check
        let ref_parts: Vec<&str> = self.redis_ref.split('/').collect();
        if ref_parts.len() != 2
            || ref_parts[0].is_empty()
            || ref_parts[1].is_empty()
            || !ref_parts
                .iter()
                .all(|p| p.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_'))
        {
            return Some(format!(
                "Invalid redis_ref '{}': must be 'namespace/name' (alphanumeric, '-', '_' only)",
                self.redis_ref
            ));
        }

        if self.policies.is_empty() {
            return Some("At least one rate limit policy is required".to_string());
        }

        if self.policies.len() > 10 {
            return Some(format!("Too many policies ({}): max 10 allowed", self.policies.len()));
        }

        for (i, policy) in self.policies.iter().enumerate() {
            if let Err(error) = validate_rate_limit_units(policy.rate) {
                return Some(format!("policies[{i}].rate {error}"));
            }
            match parse_duration(&policy.interval) {
                Err(e) => return Some(format!("policies[{i}].interval: {e}")),
                Ok(duration) => {
                    if let Err(error) = validate_rate_limit_window(duration) {
                        return Some(format!("policies[{i}].interval {error}"));
                    }
                }
            }
        }

        if !crate::http_status::is_valid_final_http_status(self.reject_status) {
            return Some(format!("Invalid reject_status: {}", self.reject_status));
        }

        if let Some(error) = self.header_names.as_ref().and_then(LimitHeaderNames::validation_error) {
            return Some(error);
        }

        if self.key_prefix.is_empty() {
            return Some("key_prefix cannot be empty".to_string());
        }
        if !self.key_prefix.ends_with(':') {
            return Some(format!("key_prefix '{}' must end with ':'", self.key_prefix));
        }
        if !self.key_prefix.chars().all(|c| c.is_ascii() && !c.is_ascii_control()) {
            return Some("key_prefix must contain only printable ASCII".to_string());
        }

        for (i, key) in self.key.iter().enumerate() {
            if let Some(name) = key.name() {
                if name.is_empty() {
                    return Some(format!(
                        "key[{}].name cannot be empty for type {:?}",
                        i,
                        key.source_type()
                    ));
                }
            }
        }

        if let Err(error) = validate_rate_limit_units(u64::from(self.cost)) {
            return Some(format!("cost {error}"));
        }

        if self.max_key_len < 64 {
            return Some(format!("max_key_len {} too small (min 64)", self.max_key_len));
        }
        if self.max_key_len > 65536 {
            return Some(format!("max_key_len {} too large (max 65536 = 64KB)", self.max_key_len));
        }

        if let Some(rt) = &self.request_timeout {
            match parse_duration(rt) {
                Err(e) => return Some(format!("request_timeout: {}", e)),
                // A zero timeout would make tokio::time::timeout elapse immediately
                // on every request, silently bypassing the Redis check (always
                // routing to on_redis_failure). Reject it.
                Ok(d) if d.is_zero() => return Some("request_timeout must be > 0".to_string()),
                Ok(_) => {}
            }
        }

        None
    }

    fn validate_inner(&mut self) -> Result<(), String> {
        if let Some(err) = self.check_fields() {
            return Err(err);
        }

        // All field checks passed; do the compile-only side effects.
        for policy in self.policies.iter_mut() {
            let dur = parse_duration(&policy.interval).expect("interval pre-validated by check_fields");
            policy.interval_duration = Some(dur);
        }

        self.request_timeout_duration = self
            .request_timeout
            .as_ref()
            .map(|rt| parse_duration(rt).expect("request_timeout pre-validated by check_fields"));

        Ok(())
    }
}

// ========== PluginValidation trait impl ==========

use crate::resources::edgion_plugins::PluginValidation;

impl PluginValidation for RateLimitRedisConfig {
    fn validation_error(&self) -> Option<String> {
        self.check_fields()
    }
}

// ========== Tests ==========

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/rate_limit_redis_test.rs"]
mod tests;
