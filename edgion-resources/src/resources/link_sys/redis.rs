//! Redis client configuration types

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::resources::common::{SecretSlot, TlsConfig};

/// Redis client configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisClientConfig {
    /// Redis server endpoints (e.g., "redis://127.0.0.1:6379")
    pub endpoints: Vec<String>,

    /// Authentication configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<RedisAuth>,

    /// Database number (0-15 for standard Redis)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db: Option<i32>,

    /// Timeout configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<RedisTimeout>,

    /// Connection pool configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<RedisPool>,

    /// Retry configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RedisRetry>,

    /// Redis topology configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topology: Option<RedisTopology>,

    /// TLS configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsConfig>,

    /// Observability configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observability: Option<RedisObservability>,
}

/// Redis authentication configuration.
///
/// All credentials MUST be stored in a Kubernetes Secret and referenced via `secretRef`.
/// Inline username/password fields have been removed (breaking change).
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RedisAuth {
    /// Embedded Secret slot (`secretRef` + resolved `secret`).
    /// `#[serde(flatten)]` keeps the YAML form unchanged at the parent level.
    #[serde(flatten)]
    pub slot: SecretSlot,
}

/// Redis timeout configuration (in milliseconds)
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisTimeout {
    /// Connection timeout in milliseconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect: Option<u64>,

    /// Read timeout in milliseconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read: Option<u64>,

    /// Write timeout in milliseconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write: Option<u64>,
}

/// Redis connection pool configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisPool {
    /// Maximum pool size
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u32>,

    /// Minimum idle connections
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_idle: Option<u32>,
}

/// Redis retry configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisRetry {
    /// Maximum number of retries
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,

    /// Backoff configuration (e.g., "exponential", "linear", "fixed")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff: Option<RedisBackoff>,
}

/// Redis backoff strategy
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisBackoff {
    /// Backoff strategy type
    #[serde(rename = "type")]
    pub backoff_type: String,

    /// Initial delay in milliseconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_delay: Option<u64>,

    /// Maximum delay in milliseconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_delay: Option<u64>,

    /// Multiplier for exponential backoff
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiplier: Option<f64>,
}

/// Redis topology configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisTopology {
    /// Topology mode
    pub mode: RedisTopologyMode,

    /// Sentinel configuration (only for sentinel mode)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sentinel: Option<RedisSentinel>,

    /// Cluster configuration (only for cluster mode)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster: Option<RedisCluster>,
}

/// Redis topology mode
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RedisTopologyMode {
    Standalone,
    Sentinel,
    Cluster,
}

/// Redis Sentinel configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisSentinel {
    /// Master name in Sentinel configuration
    pub master_name: String,

    /// Sentinel endpoints
    pub sentinels: Vec<String>,

    /// Sentinel password
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    pub password: Option<String>,
}

/// Redis Cluster configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisCluster {
    /// Read from replicas
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_from_replicas: Option<bool>,

    /// Maximum redirects
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_redirects: Option<u32>,
}

/// Redis observability configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisObservability {
    /// Enable metrics collection
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<RedisMetrics>,

    /// Logging configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logging: Option<RedisLogging>,
}

/// Redis metrics configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisMetrics {
    /// Enable metrics
    #[serde(default)]
    pub enabled: bool,

    /// Metrics labels
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<HashMap<String, String>>,
}

/// Redis logging configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisLogging {
    /// Enable logging
    #[serde(default)]
    pub enabled: bool,

    /// Log level (e.g., "debug", "info", "warn", "error")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,

    /// Log slow operations (threshold in milliseconds)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slow_log_threshold: Option<u64>,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redis_auth_requires_secret_ref() {
        let yaml = r#"
secretRef:
  name: redis-creds
"#;
        let auth: RedisAuth = serde_yaml::from_str(yaml).expect("should deserialize");
        assert_eq!(auth.slot.secret_ref.name, "redis-creds");
        assert!(auth.slot.secret_ref.namespace.is_none());
    }

    #[test]
    fn redis_auth_with_namespace() {
        let yaml = r#"
secretRef:
  name: redis-creds
  namespace: credentials
"#;
        let auth: RedisAuth = serde_yaml::from_str(yaml).expect("should deserialize");
        assert_eq!(auth.slot.secret_ref.name, "redis-creds");
        assert_eq!(auth.slot.secret_ref.namespace.as_deref(), Some("credentials"));
    }

    #[test]
    fn redis_auth_rejects_inline_password() {
        let yaml = "secretRef:\n  name: r\npassword: leaked";
        let r: Result<RedisAuth, _> = serde_yaml::from_str(yaml);
        assert!(r.is_err(), "inline password must be rejected by deny_unknown_fields");
    }

    #[test]
    fn redis_auth_rejects_inline_username() {
        let yaml = "secretRef:\n  name: r\nusername: admin";
        let r: Result<RedisAuth, _> = serde_yaml::from_str(yaml);
        assert!(r.is_err(), "inline username must be rejected by deny_unknown_fields");
    }

    #[test]
    fn redis_auth_rejects_missing_secret_ref() {
        let yaml = "{}";
        let r: Result<RedisAuth, _> = serde_yaml::from_str(yaml);
        assert!(r.is_err(), "secretRef is a required field");
    }

    #[test]
    fn redis_auth_yaml_roundtrip_unchanged() {
        // YAML shape must remain identical before and after refactoring (#[serde(flatten)] verification)
        let yaml = "secretRef:\n  name: redis-creds\n  namespace: default";
        let auth: super::RedisAuth = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(auth.slot.secret_ref.name, "redis-creds");
        assert_eq!(auth.slot.secret_ref.namespace.as_deref(), Some("default"));
        assert!(auth.slot.secret.is_none());
    }

    #[test]
    fn redis_auth_rejects_unknown_field() {
        let yaml = "secretRef:\n  name: x\nbogus: 1";
        let r: Result<super::RedisAuth, _> = serde_yaml::from_str(yaml);
        assert!(r.is_err());
    }
}
