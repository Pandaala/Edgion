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
//! - Lazy script loading via tokio::sync::OnceCell
//!
//! ## Complementary to RateLimit:
//! - RateLimit: in-process CMS, ~nanosecond, approximate, per-instance
//! - RateLimitRedis: Redis-backed, ~0.1-1ms, precise, cluster-wide

use crate::types::common::KeyGet;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::rate_limit::{LimitHeaderNames, OnMissingKey};

// ========== Rate Limit Algorithm ==========

/// Rate limiting algorithm selection.
///
/// Each algorithm trades off between accuracy, burst behavior, and Redis cost.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
pub enum RateLimitAlgorithm {
    /// Sliding window counter (default) — best accuracy/performance balance.
    ///
    /// Uses weighted average of current + previous fixed window.
    /// Like APISIX's default algorithm. Prevents boundary burst.
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
    /// Like Kong's default algorithm.
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
    pub rate: u64,

    /// Time interval (e.g., "1s", "1m", "1h", "1d").
    /// Uses the same parse_duration() as RateLimit.
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
///     - type: clientIp
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

    /// Key extraction sources (same KeyGet as RateLimit).
    ///
    /// Multiple keys are combined with "_" separator.
    /// Default: [] (empty, will use ClientIp at runtime if empty)
    #[serde(default)]
    pub key: Vec<KeyGet>,

    /// Behavior when key cannot be extracted (default: Allow)
    #[serde(default)]
    pub on_missing_key: OnMissingKey,

    /// Default key fallback when extraction fails
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_key: Option<String>,

    /// HTTP status code for rejected requests (default: 429)
    #[serde(default = "default_reject_status")]
    pub reject_status: u16,

    /// Custom rejection message (JSON body)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reject_message: Option<String>,

    /// Show rate limit headers in response (default: true)
    #[serde(default = "default_true")]
    pub show_limit_headers: bool,

    /// Custom header names for rate limit responses.
    /// Reuses LimitHeaderNames from RateLimit.
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
    pub cost: u32,

    /// Maximum length of the limit key portion in Redis keys (default: 4096 bytes).
    ///
    /// Keys extracted from requests (IP, Header, Cookie, etc.) are truncated
    /// to this length before being embedded into the Redis key string.
    /// This prevents Redis OOM from maliciously long header values.
    #[serde(default = "default_max_key_len")]
    pub max_key_len: usize,

    // --- Runtime fields (skip serialization) ---
    /// Validation error (runtime only)
    #[serde(skip)]
    #[schemars(skip)]
    pub validation_error: Option<String>,
}

// ========== Default Functions ==========

fn default_reject_status() -> u16 {
    429
}

fn default_true() -> bool {
    true
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
            on_missing_key: OnMissingKey::default(),
            default_key: None,
            reject_status: default_reject_status(),
            reject_message: None,
            show_limit_headers: true,
            header_names: None,
            key_prefix: default_key_prefix(),
            on_redis_failure: OnRedisFailure::default(),
            cost: default_cost(),
            max_key_len: default_max_key_len(),
            validation_error: None,
        }
    }
}

// ========== Validation ==========

impl RateLimitRedisConfig {
    /// Validate and compile the configuration.
    pub fn validate(&mut self) {
        if let Err(e) = self.validate_inner() {
            self.validation_error = Some(e);
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

    /// Get custom header names (if configured).
    pub fn get_header_names(&self) -> Option<&LimitHeaderNames> {
        self.header_names.as_ref()
    }

    fn validate_inner(&mut self) -> Result<(), String> {
        // redis_ref format: "namespace/name" — must be exactly two segments
        let ref_parts: Vec<&str> = self.redis_ref.split('/').collect();
        if ref_parts.len() != 2
            || ref_parts[0].is_empty()
            || ref_parts[1].is_empty()
            || !ref_parts.iter().all(|p| {
                p.chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
            })
        {
            return Err(format!(
                "Invalid redis_ref '{}': must be 'namespace/name' (alphanumeric, '-', '_' only)",
                self.redis_ref
            ));
        }

        // At least one policy
        if self.policies.is_empty() {
            return Err("At least one rate limit policy is required".to_string());
        }

        // Cap maximum policies to prevent combinatorial explosion
        if self.policies.len() > 10 {
            return Err(format!(
                "Too many policies ({}): max 10 allowed",
                self.policies.len()
            ));
        }

        // Validate each policy
        for (i, policy) in self.policies.iter_mut().enumerate() {
            if policy.rate == 0 {
                return Err(format!("policies[{}].rate must be > 0", i));
            }
            let dur = super::rate_limit::parse_duration(&policy.interval)
                .map_err(|e| format!("policies[{}].interval: {}", i, e))?;
            // Minimum interval to prevent excessive Redis writes
            if dur < Duration::from_millis(100) {
                return Err(format!(
                    "policies[{}].interval too small (< 100ms): could overwhelm Redis",
                    i
                ));
            }
            policy.interval_duration = Some(dur);
        }

        // Validate reject_status
        if self.reject_status < 100 || self.reject_status >= 600 {
            return Err(format!("Invalid reject_status: {}", self.reject_status));
        }

        // Validate key_prefix: must be non-empty, end with ':', ASCII printable only
        if self.key_prefix.is_empty() {
            return Err("key_prefix cannot be empty".to_string());
        }
        if !self.key_prefix.ends_with(':') {
            return Err(format!(
                "key_prefix '{}' must end with ':'",
                self.key_prefix
            ));
        }
        if !self
            .key_prefix
            .chars()
            .all(|c| c.is_ascii() && !c.is_ascii_control())
        {
            return Err("key_prefix must contain only printable ASCII".to_string());
        }

        // Validate key sources
        for (i, key) in self.key.iter().enumerate() {
            if let Some(name) = key.name() {
                if name.is_empty() {
                    return Err(format!(
                        "key[{}].name cannot be empty for type {:?}",
                        i,
                        key.source_type()
                    ));
                }
            }
        }

        // Validate cost
        if self.cost == 0 {
            return Err("cost must be > 0".to_string());
        }

        // Validate max_key_len: reasonable bounds [64, 65536]
        if self.max_key_len < 64 {
            return Err(format!(
                "max_key_len {} too small (min 64)",
                self.max_key_len
            ));
        }
        if self.max_key_len > 65536 {
            return Err(format!(
                "max_key_len {} too large (max 65536 = 64KB)",
                self.max_key_len
            ));
        }

        Ok(())
    }
}

// ========== Tests ==========

#[cfg(test)]
mod tests {
    use super::*;

    fn create_valid_config() -> RateLimitRedisConfig {
        RateLimitRedisConfig {
            redis_ref: "default/shared-redis".to_string(),
            policies: vec![RateLimitPolicy {
                rate: 100,
                interval: "1s".to_string(),
                algorithm: RateLimitAlgorithm::default(),
                interval_duration: None,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn test_default_config() {
        let config = RateLimitRedisConfig::default();
        assert_eq!(config.reject_status, 429);
        assert!(config.show_limit_headers);
        assert_eq!(config.key_prefix, "edgion:rl:");
        assert_eq!(config.cost, 1);
        assert_eq!(config.max_key_len, 4096);
        assert_eq!(config.on_redis_failure, OnRedisFailure::Allow);
    }

    #[test]
    fn test_valid_config() {
        let mut config = create_valid_config();
        config.validate();
        assert!(config.is_valid());
    }

    #[test]
    fn test_invalid_redis_ref_empty() {
        let mut config = create_valid_config();
        config.redis_ref = "".to_string();
        config.validate();
        assert!(!config.is_valid());
        assert!(config.get_validation_error().unwrap().contains("redis_ref"));
    }

    #[test]
    fn test_invalid_redis_ref_no_slash() {
        let mut config = create_valid_config();
        config.redis_ref = "shared-redis".to_string();
        config.validate();
        assert!(!config.is_valid());
        assert!(config.get_validation_error().unwrap().contains("redis_ref"));
    }

    #[test]
    fn test_invalid_redis_ref_special_chars() {
        let mut config = create_valid_config();
        config.redis_ref = "default/redis@cluster".to_string();
        config.validate();
        assert!(!config.is_valid());
    }

    #[test]
    fn test_empty_policies() {
        let mut config = RateLimitRedisConfig {
            redis_ref: "default/redis".to_string(),
            policies: vec![],
            ..Default::default()
        };
        config.validate();
        assert!(!config.is_valid());
        assert!(config
            .get_validation_error()
            .unwrap()
            .contains("At least one"));
    }

    #[test]
    fn test_too_many_policies() {
        let mut config = RateLimitRedisConfig {
            redis_ref: "default/redis".to_string(),
            policies: (0..11)
                .map(|_| RateLimitPolicy {
                    rate: 100,
                    interval: "1s".to_string(),
                    algorithm: RateLimitAlgorithm::default(),
                    interval_duration: None,
                })
                .collect(),
            ..Default::default()
        };
        config.validate();
        assert!(!config.is_valid());
        assert!(config.get_validation_error().unwrap().contains("Too many"));
    }

    #[test]
    fn test_zero_rate_policy() {
        let mut config = RateLimitRedisConfig {
            redis_ref: "default/redis".to_string(),
            policies: vec![RateLimitPolicy {
                rate: 0,
                interval: "1s".to_string(),
                algorithm: RateLimitAlgorithm::default(),
                interval_duration: None,
            }],
            ..Default::default()
        };
        config.validate();
        assert!(!config.is_valid());
        assert!(config.get_validation_error().unwrap().contains("rate must be > 0"));
    }

    #[test]
    fn test_interval_too_small() {
        let mut config = RateLimitRedisConfig {
            redis_ref: "default/redis".to_string(),
            policies: vec![RateLimitPolicy {
                rate: 100,
                interval: "50ms".to_string(),
                algorithm: RateLimitAlgorithm::default(),
                interval_duration: None,
            }],
            ..Default::default()
        };
        config.validate();
        assert!(!config.is_valid());
        assert!(config
            .get_validation_error()
            .unwrap()
            .contains("too small"));
    }

    #[test]
    fn test_invalid_key_prefix_no_colon() {
        let mut config = create_valid_config();
        config.key_prefix = "edgion_rl".to_string();
        config.validate();
        assert!(!config.is_valid());
        assert!(config
            .get_validation_error()
            .unwrap()
            .contains("must end with ':'"));
    }

    #[test]
    fn test_invalid_key_prefix_empty() {
        let mut config = create_valid_config();
        config.key_prefix = "".to_string();
        config.validate();
        assert!(!config.is_valid());
    }

    #[test]
    fn test_zero_cost() {
        let mut config = create_valid_config();
        config.cost = 0;
        config.validate();
        assert!(!config.is_valid());
        assert!(config.get_validation_error().unwrap().contains("cost"));
    }

    #[test]
    fn test_max_key_len_too_small() {
        let mut config = create_valid_config();
        config.max_key_len = 32;
        config.validate();
        assert!(!config.is_valid());
        assert!(config
            .get_validation_error()
            .unwrap()
            .contains("max_key_len"));
    }

    #[test]
    fn test_max_key_len_too_large() {
        let mut config = create_valid_config();
        config.max_key_len = 100_000;
        config.validate();
        assert!(!config.is_valid());
    }

    #[test]
    fn test_multi_policy_config() {
        let mut config = RateLimitRedisConfig {
            redis_ref: "prod/redis-cluster".to_string(),
            policies: vec![
                RateLimitPolicy {
                    rate: 50,
                    interval: "1s".to_string(),
                    algorithm: RateLimitAlgorithm::SlidingWindow,
                    interval_duration: None,
                },
                RateLimitPolicy {
                    rate: 5000,
                    interval: "1h".to_string(),
                    algorithm: RateLimitAlgorithm::FixedWindow,
                    interval_duration: None,
                },
            ],
            ..Default::default()
        };
        config.validate();
        assert!(config.is_valid());
        assert_eq!(
            config.policies[0].interval_duration,
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            config.policies[1].interval_duration,
            Some(Duration::from_secs(3600))
        );
    }

    #[test]
    fn test_algorithm_key_prefix() {
        assert_eq!(RateLimitAlgorithm::SlidingWindow.key_prefix(), "sw");
        assert_eq!(RateLimitAlgorithm::FixedWindow.key_prefix(), "fw");
        assert_eq!(RateLimitAlgorithm::TokenBucket.key_prefix(), "tb");
    }

    #[test]
    fn test_yaml_deserialization_basic() {
        let yaml = r#"
redisRef: "default/shared-redis"
key:
  - type: clientIp
policies:
  - rate: 100
    interval: "1s"
"#;
        let config: RateLimitRedisConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.redis_ref, "default/shared-redis");
        assert_eq!(config.policies.len(), 1);
        assert_eq!(config.policies[0].rate, 100);
        assert_eq!(config.policies[0].algorithm, RateLimitAlgorithm::SlidingWindow);
    }

    #[test]
    fn test_yaml_deserialization_multi_policy() {
        let yaml = r#"
redisRef: "prod/redis"
policies:
  - rate: 50
    interval: "1s"
    algorithm: SlidingWindow
  - rate: 5000
    interval: "1h"
    algorithm: FixedWindow
  - rate: 100
    interval: "1m"
    algorithm: TokenBucket
onRedisFailure: Deny
onMissingKey: Deny
cost: 5
maxKeyLen: 16384
keyPrefix: "myapp:rl:"
"#;
        let config: RateLimitRedisConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.policies.len(), 3);
        assert_eq!(config.policies[0].algorithm, RateLimitAlgorithm::SlidingWindow);
        assert_eq!(config.policies[1].algorithm, RateLimitAlgorithm::FixedWindow);
        assert_eq!(config.policies[2].algorithm, RateLimitAlgorithm::TokenBucket);
        assert_eq!(config.on_redis_failure, OnRedisFailure::Deny);
        assert_eq!(config.on_missing_key, OnMissingKey::Deny);
        assert_eq!(config.cost, 5);
        assert_eq!(config.max_key_len, 16384);
        assert_eq!(config.key_prefix, "myapp:rl:");
    }

    #[test]
    fn test_invalid_reject_status() {
        let mut config = create_valid_config();
        config.reject_status = 600;
        config.validate();
        assert!(!config.is_valid());
        assert!(config
            .get_validation_error()
            .unwrap()
            .contains("reject_status"));
    }

    #[test]
    fn test_empty_key_name() {
        let mut config = create_valid_config();
        config.key = vec![KeyGet::Header {
            name: "".to_string(),
        }];
        config.validate();
        assert!(!config.is_valid());
        assert!(config
            .get_validation_error()
            .unwrap()
            .contains("key[0].name"));
    }

    #[test]
    fn test_on_redis_failure_default() {
        let config = RateLimitRedisConfig::default();
        assert_eq!(config.on_redis_failure, OnRedisFailure::Allow);
    }
}
