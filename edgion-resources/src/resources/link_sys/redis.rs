//! Redis client configuration types

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use url::{Host, Url};

use crate::duration::parse_duration;
use crate::resources::common::{SecretSlot, TlsConfig};

pub const REDIS_DEFAULT_POOL_SIZE: u32 = 8;
pub const REDIS_MAX_POOL_SIZE: u32 = 64;
pub const REDIS_DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const REDIS_MAX_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const REDIS_DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
pub const REDIS_MAX_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
pub const REDIS_MAX_REDIRECTS: u32 = 64;
const REDIS_DEFAULT_PORT: u16 = 6379;
/// Sentinel's standard port; a portless `topology.sentinel.sentinels` entry uses it.
const REDIS_SENTINEL_DEFAULT_PORT: u16 = 26379;

/// Validated Redis transport selected by an endpoint scheme.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RedisTransport {
    Plaintext,
    Tls,
}

/// Parsed active endpoint shared by Controller validation and Gateway mapping.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedRedisEndpoint {
    pub host: String,
    pub port: u16,
    pub transport: RedisTransport,
}

/// Redis client configuration
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedisClientConfig {
    /// Standalone or Cluster Redis endpoints. Sentinel uses its nested list.
    #[serde(default)]
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

        if let Err(endpoint_errors) = self.parsed_active_endpoints() {
            errors.extend(endpoint_errors);
        }

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

    /// Validate the selected topology and parse every active endpoint.
    ///
    /// Errors identify only the field and list index. The original value is
    /// deliberately omitted because an operator can accidentally place
    /// credentials in an endpoint URL and validation errors reach status and
    /// logs.
    pub fn parsed_active_endpoints(&self) -> Result<Vec<ParsedRedisEndpoint>, Vec<String>> {
        let mut errors = Vec::new();
        let (field, endpoints) = match self.topology.as_ref() {
            None => {
                validate_standalone_shape(self, &mut errors);
                ("endpoints", self.endpoints.as_slice())
            }
            Some(topology) => match &topology.mode {
                RedisTopologyMode::Standalone => {
                    validate_standalone_shape(self, &mut errors);
                    ("endpoints", self.endpoints.as_slice())
                }
                RedisTopologyMode::Cluster => {
                    if self.endpoints.is_empty() {
                        errors.push("Redis cluster mode requires at least one endpoint".to_string());
                    }
                    if topology.sentinel.is_some() {
                        errors.push("Redis cluster mode must not configure topology.sentinel".to_string());
                    }
                    ("endpoints", self.endpoints.as_slice())
                }
                RedisTopologyMode::Sentinel => {
                    if !self.endpoints.is_empty() {
                        errors.push("Redis sentinel mode must not configure top-level endpoints".to_string());
                    }
                    if topology.cluster.is_some() {
                        errors.push("Redis sentinel mode must not configure topology.cluster".to_string());
                    }
                    match topology.sentinel.as_ref() {
                        Some(sentinel) => {
                            if sentinel.master_name.trim().is_empty() {
                                errors.push("Redis topology.sentinel.masterName must not be empty".to_string());
                            }
                            if sentinel.sentinels.is_empty() {
                                errors.push(
                                    "Redis sentinel mode requires at least one topology.sentinel.sentinels endpoint"
                                        .to_string(),
                                );
                            }
                            ("topology.sentinel.sentinels", sentinel.sentinels.as_slice())
                        }
                        None => {
                            errors.push("Redis sentinel mode requires topology.sentinel".to_string());
                            ("topology.sentinel.sentinels", &[] as &[String])
                        }
                    }
                }
            },
        };

        let default_port = if field == "topology.sentinel.sentinels" {
            REDIS_SENTINEL_DEFAULT_PORT
        } else {
            REDIS_DEFAULT_PORT
        };
        let mut parsed = Vec::with_capacity(endpoints.len());
        for (index, endpoint) in endpoints.iter().enumerate() {
            match parse_redis_endpoint(endpoint, default_port) {
                Ok(endpoint) => parsed.push(endpoint),
                Err(reason) => errors.push(format!("Redis {field}[{index}] is invalid: {reason}")),
            }
        }

        let explicit_tls = self.tls.as_ref().is_some_and(|tls| tls.enabled);
        if !explicit_tls {
            let has_plaintext = parsed
                .iter()
                .any(|endpoint| endpoint.transport == RedisTransport::Plaintext);
            let has_tls = parsed.iter().any(|endpoint| endpoint.transport == RedisTransport::Tls);
            if has_plaintext && has_tls {
                errors.push(
                    "Redis active endpoints cannot mix TLS and plaintext transports unless tls.enabled=true"
                        .to_string(),
                );
            }
        }

        if errors.is_empty() {
            Ok(parsed)
        } else {
            Err(errors)
        }
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

fn validate_standalone_shape(config: &RedisClientConfig, errors: &mut Vec<String>) {
    if config.endpoints.len() != 1 {
        errors.push("Redis standalone mode requires exactly one endpoint".to_string());
    }
    if let Some(topology) = &config.topology {
        if topology.sentinel.is_some() {
            errors.push("Redis standalone mode must not configure topology.sentinel".to_string());
        }
        if topology.cluster.is_some() {
            errors.push("Redis standalone mode must not configure topology.cluster".to_string());
        }
    }
}

fn parse_redis_endpoint(value: &str, default_port: u16) -> Result<ParsedRedisEndpoint, &'static str> {
    if value.is_empty() || value.trim() != value {
        return Err("must not be empty or contain surrounding whitespace");
    }

    let (transport, authority) = if let Some(rest) = value.strip_prefix("redis://") {
        (RedisTransport::Plaintext, rest)
    } else if let Some(rest) = value.strip_prefix("rediss://") {
        (RedisTransport::Tls, rest)
    } else if value.contains("://") {
        return Err("scheme must be redis or rediss");
    } else {
        (RedisTransport::Plaintext, value)
    };
    let authority_end = authority.find(['/', '?', '#']).unwrap_or(authority.len());
    let host_and_port = &authority[..authority_end];
    if host_and_port.ends_with(':') {
        return Err("port must be between 1 and 65535");
    }
    let suffix = &authority[authority_end..];
    if suffix.starts_with('/') {
        let path_end = suffix.find(['?', '#']).unwrap_or(suffix.len());
        if &suffix[..path_end] != "/" {
            return Err("paths are not supported");
        }
    }
    let url = Url::parse(&format!("redis://{authority}")).map_err(|_| "must contain a valid host and optional port")?;

    if !url.username().is_empty() || url.password().is_some() {
        return Err("user information is not supported");
    }
    if !matches!(url.path(), "" | "/") {
        return Err("paths are not supported");
    }
    if url.query().is_some() {
        return Err("queries are not supported");
    }
    if url.fragment().is_some() {
        return Err("fragments are not supported");
    }

    let host = match url.host() {
        Some(Host::Domain(host)) if !host.is_empty() => host.to_string(),
        Some(Host::Ipv4(host)) => host.to_string(),
        Some(Host::Ipv6(host)) => host.to_string(),
        _ => return Err("host must not be empty"),
    };
    let port = url.port().unwrap_or(default_port);
    if port == 0 {
        return Err("port must be between 1 and 65535");
    }

    Ok(ParsedRedisEndpoint { host, port, transport })
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
#[path = "../../../tests/unit/resources/link_sys/redis_test.rs"]
mod tests;
