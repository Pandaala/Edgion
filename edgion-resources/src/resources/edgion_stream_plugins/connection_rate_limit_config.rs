//! L4 connection-rate-limit plugin config for `EdgionStreamPlugin::ConnectionRateLimit`.
//!
//! Two independently optional dimensions: `perListener` (process-level backstop;
//! a local in-process gate plus an optional Redis fleet-wide cap) and
//! `perSourceIp` (Redis-backed per-source throttle). Per-dimension `onRedisFailure`
//! defaults differ: perListener => Deny (fail-close), perSourceIp => Allow (fail-open).

use crate::duration::parse_duration;
use crate::resources::edgion_plugins::{OnRedisFailure, PluginValidation, RateLimitAlgorithm};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;

fn default_key_prefix() -> String {
    "edgion:crl:".to_string()
}
fn default_on_redis_failure_deny() -> OnRedisFailure {
    OnRedisFailure::Deny
}
fn default_on_redis_failure_allow() -> OnRedisFailure {
    OnRedisFailure::Allow
}

/// Per-listener connection-rate limit: enforced by a local in-process gate, and
/// optionally also as a fleet-wide cap in Redis when `redisDistributed` is set.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PerListenerLimit {
    /// Max new connections per interval for this listener port.
    #[schemars(range(min = 1))]
    pub rate: u64,
    /// Interval duration string (e.g. "1s", "500ms").
    pub interval: String,
    /// Also enforce a fleet-wide cap in Redis (requires `redisRef`). Default false.
    #[serde(default)]
    pub redis_distributed: bool,
    /// Redis-failure policy for the distributed cap. Default Deny (fail-close).
    #[serde(default = "default_on_redis_failure_deny")]
    pub on_redis_failure: OnRedisFailure,
    /// Parsed interval (runtime only).
    #[serde(skip)]
    #[schemars(skip)]
    pub interval_duration: Option<Duration>,
}

/// Per-source-IP connection-rate limit, enforced via Redis.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PerSourceIpLimit {
    /// Max new connections per interval per source IP.
    #[schemars(range(min = 1))]
    pub rate: u64,
    /// Interval duration string (e.g. "1s", "500ms").
    pub interval: String,
    /// Redis-failure policy. Default Allow (fail-open) to avoid collateral blocking.
    #[serde(default = "default_on_redis_failure_allow")]
    pub on_redis_failure: OnRedisFailure,
    /// Parsed interval (runtime only).
    #[serde(skip)]
    #[schemars(skip)]
    pub interval_duration: Option<Duration>,
}

/// L4 connection-rate-limit plugin configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionRateLimitConfig {
    /// Redis LinkSys ref ("namespace/name"). Required only when a Redis-backed
    /// dimension is enabled (perSourceIp, or perListener with redisDistributed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redis_ref: Option<String>,
    /// Rate-limit algorithm. Default SlidingWindow.
    #[serde(default)]
    pub algorithm: RateLimitAlgorithm,
    /// Redis key prefix (must end with ':'). Default "edgion:crl:".
    #[serde(default = "default_key_prefix")]
    pub key_prefix: String,
    /// Per-listener limiter (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_listener: Option<PerListenerLimit>,
    /// Per-source-IP limiter (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_source_ip: Option<PerSourceIpLimit>,
}

impl ConnectionRateLimitConfig {
    /// True if any dimension needs Redis (perSourceIp always; perListener only when distributed).
    pub fn needs_redis(&self) -> bool {
        self.per_source_ip.is_some() || self.per_listener.as_ref().is_some_and(|l| l.redis_distributed)
    }

    fn check_rate_interval(label: &str, rate: u64, interval: &str) -> Option<String> {
        if rate == 0 {
            return Some(format!("{label}.rate must be > 0"));
        }
        match parse_duration(interval) {
            Err(e) => Some(format!("{label}.interval: {e}")),
            Ok(d) if d < Duration::from_millis(100) => {
                Some(format!("{label}.interval too small (< 100ms): could overwhelm Redis"))
            }
            Ok(_) => None,
        }
    }

    fn check_redis_ref(&self) -> Option<String> {
        let Some(r) = &self.redis_ref else {
            return Some("redisRef is required when a Redis-backed dimension is enabled".to_string());
        };
        let parts: Vec<&str> = r.split('/').collect();
        let ok = parts.len() == 2
            && !parts[0].is_empty()
            && !parts[1].is_empty()
            && parts
                .iter()
                .all(|p| p.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_'));
        if ok {
            None
        } else {
            Some(format!(
                "Invalid redisRef '{r}': must be 'namespace/name' (alphanumeric, '-', '_' only)"
            ))
        }
    }
}

impl PluginValidation for ConnectionRateLimitConfig {
    fn validation_error(&self) -> Option<String> {
        if self.per_listener.is_none() && self.per_source_ip.is_none() {
            return Some("At least one of 'perListener' or 'perSourceIp' must be set".to_string());
        }
        if let Some(l) = &self.per_listener {
            if let Some(e) = Self::check_rate_interval("perListener", l.rate, &l.interval) {
                return Some(e);
            }
        }
        if let Some(s) = &self.per_source_ip {
            if let Some(e) = Self::check_rate_interval("perSourceIp", s.rate, &s.interval) {
                return Some(e);
            }
        }
        if self.needs_redis() {
            if let Some(e) = self.check_redis_ref() {
                return Some(e);
            }
        }
        if self.key_prefix.is_empty() || !self.key_prefix.ends_with(':') {
            return Some("key_prefix must be non-empty and end with ':'".to_string());
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip_dim(rate: u64, interval: &str) -> PerSourceIpLimit {
        PerSourceIpLimit {
            rate,
            interval: interval.to_string(),
            on_redis_failure: OnRedisFailure::Allow,
            interval_duration: None,
        }
    }

    #[test]
    fn rejects_when_no_dimension_set() {
        let cfg = ConnectionRateLimitConfig {
            redis_ref: Some("default/redis".to_string()),
            algorithm: RateLimitAlgorithm::SlidingWindow,
            key_prefix: "edgion:crl:".to_string(),
            per_listener: None,
            per_source_ip: None,
        };
        assert_eq!(
            cfg.validation_error(),
            Some("At least one of 'perListener' or 'perSourceIp' must be set".to_string())
        );
    }

    #[test]
    fn rejects_zero_rate() {
        let cfg = ConnectionRateLimitConfig {
            redis_ref: Some("default/redis".to_string()),
            algorithm: RateLimitAlgorithm::SlidingWindow,
            key_prefix: "edgion:crl:".to_string(),
            per_listener: None,
            per_source_ip: Some(ip_dim(0, "1s")),
        };
        assert!(cfg.validation_error().unwrap().contains("rate must be > 0"));
    }

    #[test]
    fn rejects_interval_below_100ms() {
        let cfg = ConnectionRateLimitConfig {
            redis_ref: Some("default/redis".to_string()),
            algorithm: RateLimitAlgorithm::SlidingWindow,
            key_prefix: "edgion:crl:".to_string(),
            per_listener: None,
            per_source_ip: Some(ip_dim(10, "50ms")),
        };
        assert!(cfg.validation_error().unwrap().contains("< 100ms"));
    }

    #[test]
    fn requires_redis_ref_when_redis_dimension_enabled() {
        let cfg = ConnectionRateLimitConfig {
            redis_ref: None,
            algorithm: RateLimitAlgorithm::SlidingWindow,
            key_prefix: "edgion:crl:".to_string(),
            per_listener: None,
            per_source_ip: Some(ip_dim(10, "1s")),
        };
        assert!(cfg.validation_error().unwrap().contains("redisRef is required"));
    }

    #[test]
    fn rejects_bad_redis_ref_format() {
        let cfg = ConnectionRateLimitConfig {
            redis_ref: Some("no-slash".to_string()),
            algorithm: RateLimitAlgorithm::SlidingWindow,
            key_prefix: "edgion:crl:".to_string(),
            per_listener: None,
            per_source_ip: Some(ip_dim(10, "1s")),
        };
        assert!(cfg.validation_error().unwrap().contains("must be 'namespace/name'"));
    }

    #[test]
    fn accepts_local_only_per_listener_without_redis() {
        let cfg = ConnectionRateLimitConfig {
            redis_ref: None,
            algorithm: RateLimitAlgorithm::SlidingWindow,
            key_prefix: "edgion:crl:".to_string(),
            per_listener: Some(PerListenerLimit {
                rate: 10000,
                interval: "1s".to_string(),
                redis_distributed: false,
                on_redis_failure: OnRedisFailure::Deny,
                interval_duration: None,
            }),
            per_source_ip: None,
        };
        assert_eq!(cfg.validation_error(), None);
    }

    #[test]
    fn defaults_per_dimension_on_redis_failure() {
        let yaml = r#"
redisRef: "default/redis"
perListener:
  rate: 10000
  interval: "1s"
  redisDistributed: true
perSourceIp:
  rate: 50
  interval: "1s"
"#;
        let cfg: ConnectionRateLimitConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.per_listener.unwrap().on_redis_failure, OnRedisFailure::Deny);
        assert_eq!(cfg.per_source_ip.unwrap().on_redis_failure, OnRedisFailure::Allow);
    }
}
