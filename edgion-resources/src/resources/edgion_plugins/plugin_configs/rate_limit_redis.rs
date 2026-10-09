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

/// Maximum independently checked quotas and aggregate policy work per entry.
pub const MAX_REDIS_QUOTAS: usize = 16;
pub const MAX_REDIS_POLICIES: usize = 64;

/// One identity and weighted charge, checked against every configured window.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RateLimitQuota {
    #[schemars(length(min = 1, max = 32))]
    pub name: String,
    #[serde(default = "default_key_sources")]
    pub key: Vec<KeyGet>,
    #[serde(default)]
    pub on_key_miss: OnKeyMiss,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_key: Option<String>,
    #[serde(default = "default_cost")]
    #[schemars(range(min = 1, max = 1000000))]
    pub cost: u32,
    #[schemars(length(min = 1, max = 10))]
    pub policies: Vec<RateLimitPolicy>,
}

impl Default for RateLimitQuota {
    fn default() -> Self {
        Self {
            name: "default".into(),
            key: default_key_sources(),
            on_key_miss: OnKeyMiss::default(),
            default_key: None,
            cost: default_cost(),
            policies: Vec::new(),
        }
    }
}

/// Canonical cold named-quota validation, also used by computed-cost consumers.
pub fn validate_quotas(quotas: &[RateLimitQuota], response: Option<&str>) -> Option<String> {
    if quotas.is_empty() || quotas.len() > MAX_REDIS_QUOTAS {
        return Some("Expected 1..=16 quotas".into());
    }
    let mut names = std::collections::HashSet::new();
    let mut policy_count = 0;
    for quota in quotas {
        if quota.name.is_empty()
            || quota.name.len() > 32
            || !quota
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || !names.insert(quota.name.as_str())
        {
            return Some("Quota names must be unique ASCII names of 1..=32 bytes".into());
        }
        if quota.policies.is_empty() {
            return Some("At least one rate limit policy is required".into());
        }
        if quota.policies.len() > 10 {
            return Some("Too many policies: max 10 allowed per quota".into());
        }
        policy_count += quota.policies.len();
        for (i, policy) in quota.policies.iter().enumerate() {
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
        for (i, key) in quota.key.iter().enumerate() {
            if key.name().is_some_and(str::is_empty) {
                return Some(format!("key[{i}].name cannot be empty"));
            }
        }
        if let Err(error) = validate_rate_limit_units(u64::from(quota.cost)) {
            return Some(format!("cost {error}"));
        }
    }
    if policy_count > MAX_REDIS_POLICIES {
        return Some("Too many policies: max 64 allowed per entry".into());
    }
    if response.is_some_and(|name| !names.contains(name)) {
        return Some("responseQuota must name a configured quota".into());
    }
    None
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
///   quotas:
///     - name: client
///       key:
///         - type: remoteIp
///       policies:
///         - rate: 100
///           interval: "1s"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RateLimitRedisConfig {
    /// Reference to Redis LinkSys resource ("namespace/name").
    ///
    /// Must point to an existing LinkSys of type Redis.
    /// The plugin resolves the client at runtime via get_redis_client().
    pub redis_ref: String,

    /// Independently named quotas; every active quota must allow the request.
    #[schemars(length(min = 1, max = 16))]
    pub quotas: Vec<RateLimitQuota>,

    /// Success-header quota. Omitted selects the first checked quota.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_quota: Option<String>,

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
            quotas: vec![RateLimitQuota::default()],
            response_quota: None,
            reject_status: default_reject_status(),
            reject_message: None,
            show_limit_headers: true,
            header_names: None,
            key_prefix: default_key_prefix(),
            on_redis_failure: OnRedisFailure::default(),
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

        if let Some(error) = validate_quotas(&self.quotas, self.response_quota.as_deref()) {
            return Some(error);
        }
        if self.quotas.len() > 1 && self.request_timeout.is_none() {
            return Some("Multi-quota entries require requestTimeout".to_string());
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
        for policy in self.quotas.iter_mut().flat_map(|quota| quota.policies.iter_mut()) {
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
