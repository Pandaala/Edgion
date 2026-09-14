//! Redis client configuration types

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::duration::parse_duration;
use crate::resources::common::{SecretSlot, TlsConfig};

pub const REDIS_DEFAULT_POOL_SIZE: u32 = 8;
pub const REDIS_MAX_POOL_SIZE: u32 = 64;
pub const REDIS_DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const REDIS_MAX_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const REDIS_DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
pub const REDIS_MAX_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
pub const REDIS_MAX_REDIRECTS: u32 = 64;

/// Redis client configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisClientConfig {
    /// Redis server endpoints (e.g., "redis://127.0.0.1:6379")
    pub endpoints: Vec<String>,

    /// Authentication configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<RedisAuth>,

    /// Database number (0-255; the server must expose the selected database)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db: Option<i32>,

    /// Timeout configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<RedisTimeout>,

    /// Connection pool configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<RedisPool>,

    /// Redis topology configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topology: Option<RedisTopology>,

    /// TLS configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsConfig>,
}

impl RedisClientConfig {
    pub fn validation_errors(&self) -> Vec<String> {
        let mut errors = Vec::new();

        if !(0..=255).contains(&self.db.unwrap_or_default()) {
            errors.push("Redis db must be between 0 and 255".to_string());
        }

        if let Some(size) = self.pool.as_ref().and_then(|pool| pool.size) {
            if !(1..=REDIS_MAX_POOL_SIZE).contains(&size) {
                errors.push(format!("Redis pool.size must be between 1 and {REDIS_MAX_POOL_SIZE}"));
            }
        }

        if let Some(timeout) = &self.timeout {
            validate_timeout(
                timeout.connect.as_deref(),
                REDIS_MAX_CONNECT_TIMEOUT,
                "timeout.connect",
                &mut errors,
            );
            validate_timeout(
                timeout.command.as_deref(),
                REDIS_MAX_COMMAND_TIMEOUT,
                "timeout.command",
                &mut errors,
            );
        }

        if let Some(max_redirects) = self
            .topology
            .as_ref()
            .and_then(|topology| topology.cluster.as_ref())
            .and_then(|cluster| cluster.max_redirects)
        {
            if max_redirects > REDIS_MAX_REDIRECTS {
                errors.push(format!(
                    "Redis topology.cluster.maxRedirects must not exceed {REDIS_MAX_REDIRECTS}"
                ));
            }
        }

        errors
    }

    pub fn connect_timeout(&self) -> Result<Duration, String> {
        resolve_timeout(
            self.timeout.as_ref().and_then(|timeout| timeout.connect.as_deref()),
            REDIS_DEFAULT_CONNECT_TIMEOUT,
            REDIS_MAX_CONNECT_TIMEOUT,
            "timeout.connect",
        )
    }

    pub fn command_timeout(&self) -> Result<Duration, String> {
        resolve_timeout(
            self.timeout.as_ref().and_then(|timeout| timeout.command.as_deref()),
            REDIS_DEFAULT_COMMAND_TIMEOUT,
            REDIS_MAX_COMMAND_TIMEOUT,
            "timeout.command",
        )
    }
}

fn validate_timeout(value: Option<&str>, maximum: Duration, field: &str, errors: &mut Vec<String>) {
    if let Err(error) = resolve_timeout(value, Duration::from_secs(1), maximum, field) {
        errors.push(error);
    }
}

fn resolve_timeout(value: Option<&str>, default: Duration, maximum: Duration, field: &str) -> Result<Duration, String> {
    let Some(value) = value else {
        return Ok(default);
    };
    let duration = parse_duration(value).map_err(|error| format!("Redis {field} is invalid: {error}"))?;
    if duration.is_zero() {
        return Err(format!("Redis {field} must be greater than zero"));
    }
    if duration > maximum {
        return Err(format!("Redis {field} must not exceed {}ms", maximum.as_millis()));
    }
    Ok(duration)
}

/// Redis authentication configuration.
///
/// Credentials must be stored in a Kubernetes Secret and referenced via `secretRef`.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RedisAuth {
    /// Embedded Secret slot (`secretRef` + resolved `secret`).
    /// `#[serde(flatten)]` keeps the YAML form unchanged at the parent level.
    #[serde(flatten)]
    pub slot: SecretSlot,
}

/// Redis timeout configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisTimeout {
    /// Connection establishment timeout in GEP-2257 format
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub connect: Option<String>,

    /// Timeout for one Redis command in GEP-2257 format
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(pattern(crate::duration::GEP2257_PATTERN))]
    pub command: Option<String>,
}

/// Redis connection pool configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisPool {
    /// Maximum pool size
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u32>,
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
}

/// Redis Cluster configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisCluster {
    /// Maximum redirects
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_redirects: Option<u32>,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_config() -> RedisClientConfig {
        RedisClientConfig {
            endpoints: vec!["redis://redis.example.com:6379".to_string()],
            auth: None,
            db: None,
            timeout: None,
            pool: None,
            topology: None,
            tls: None,
        }
    }

    #[test]
    fn redis_configuration_contract_accepts_supported_boundaries() {
        let config = RedisClientConfig {
            db: Some(255),
            timeout: Some(RedisTimeout {
                connect: Some("10s".to_string()),
                command: Some("30s".to_string()),
            }),
            pool: Some(RedisPool {
                size: Some(REDIS_MAX_POOL_SIZE),
            }),
            topology: Some(RedisTopology {
                mode: RedisTopologyMode::Cluster,
                sentinel: None,
                cluster: Some(RedisCluster {
                    max_redirects: Some(REDIS_MAX_REDIRECTS),
                }),
            }),
            ..minimal_config()
        };

        assert!(config.validation_errors().is_empty());
        assert_eq!(config.connect_timeout().unwrap(), REDIS_MAX_CONNECT_TIMEOUT);
        assert_eq!(config.command_timeout().unwrap(), REDIS_MAX_COMMAND_TIMEOUT);
    }

    #[test]
    fn redis_configuration_contract_rejects_unsupported_values() {
        let config = RedisClientConfig {
            db: Some(256),
            timeout: Some(RedisTimeout {
                connect: Some("0s".to_string()),
                command: Some("31s".to_string()),
            }),
            pool: Some(RedisPool { size: Some(0) }),
            topology: Some(RedisTopology {
                mode: RedisTopologyMode::Cluster,
                sentinel: None,
                cluster: Some(RedisCluster {
                    max_redirects: Some(REDIS_MAX_REDIRECTS + 1),
                }),
            }),
            ..minimal_config()
        };
        let errors = config.validation_errors().join("; ");

        assert!(errors.contains("db must be between 0 and 255"));
        assert!(errors.contains("pool.size must be between 1 and 64"));
        assert!(errors.contains("timeout.connect must be greater than zero"));
        assert!(errors.contains("timeout.command must not exceed 30000ms"));
        assert!(errors.contains("maxRedirects must not exceed 64"));
    }

    #[test]
    fn redis_timeout_defaults_are_explicit_and_bounded() {
        let config = minimal_config();

        assert_eq!(config.connect_timeout().unwrap(), REDIS_DEFAULT_CONNECT_TIMEOUT);
        assert_eq!(config.command_timeout().unwrap(), REDIS_DEFAULT_COMMAND_TIMEOUT);
    }

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
        // Verify #[serde(flatten)] preserves the YAML shape at the parent level.
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

    #[test]
    fn redis_sentinel_ignores_stale_password() {
        // No deny_unknown_fields on RedisSentinel: a legacy `password:` from a
        // pre-upgrade CR must be silently ignored, not rejected (graceful upgrade).
        let yaml = "masterName: mymaster\nsentinels:\n  - redis://h:26379\npassword: legacy-no-op";
        let s: super::RedisSentinel = serde_yaml::from_str(yaml).expect("stale password must be ignored");
        assert_eq!(s.master_name, "mymaster");
        assert_eq!(s.sentinels.len(), 1);
    }
}
