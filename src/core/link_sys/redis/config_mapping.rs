//! CRD `RedisClientConfig` → fred `Config` + `Builder` mapping.
//!
//! This module bridges the CRD configuration types (serde-driven, user-facing YAML)
//! with fred's native configuration structs. The mapping follows the Library-First
//! principle: we use fred's API directly without inventing our own abstractions.

use std::time::Duration;

use anyhow::{Context, Result};
use fred::types::config::{Config, ReconnectPolicy, ServerConfig, TcpConfig};
use fred::types::Builder;

use crate::types::resources::link_sys::redis::{RedisClientConfig, RedisTopologyMode};

// ============================================================================
// Safety ceilings for connection configuration
// ============================================================================

const MAX_CONNECT_TIMEOUT_MS: u64 = 10_000;
const MAX_COMMAND_TIMEOUT_MS: u64 = 30_000;
const MAX_POOL_SIZE: usize = 64;
const DEFAULT_POOL_SIZE: usize = 8;
const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 5_000;

// ============================================================================
// Config mapping (CRD → fred Config)
// ============================================================================

/// Map `RedisClientConfig` (CRD) to fred `Config`.
///
/// This function handles:
/// - Server topology (standalone / sentinel / cluster)
/// - Authentication (username / password, secrets resolved externally)
/// - Database selection
///
/// TLS, pool, and timeout settings are applied via the `Builder` in `build_fred_pool`.
pub fn build_fred_config(crd: &RedisClientConfig) -> Result<Config> {
    let server = build_server_config(crd)?;

    let mut config = Config {
        server,
        ..Default::default()
    };

    // Database selection (standalone/sentinel only; cluster ignores this)
    if let Some(db) = crd.db {
        config.database = Some(db as u8);
    }

    // Authentication (secret_ref is resolved externally before calling this function)
    if let Some(auth) = &crd.auth {
        if let Some(pw) = &auth.password {
            config.password = Some(pw.clone());
        }
        if let Some(user) = &auth.username {
            config.username = Some(user.clone());
        }
    }

    Ok(config)
}

/// Map CRD topology to fred `ServerConfig`.
fn build_server_config(crd: &RedisClientConfig) -> Result<ServerConfig> {
    let endpoints = &crd.endpoints;
    if endpoints.is_empty() {
        anyhow::bail!("Redis endpoints list is empty");
    }

    match crd.topology.as_ref().map(|t| &t.mode) {
        // Cluster mode
        Some(RedisTopologyMode::Cluster) => {
            let hosts: Vec<(String, u16)> = endpoints
                .iter()
                .filter_map(|ep| parse_redis_url(ep).ok())
                .collect();
            if hosts.is_empty() {
                anyhow::bail!("No valid cluster endpoints could be parsed");
            }
            Ok(ServerConfig::new_clustered(hosts))
        }
        // Sentinel mode
        Some(RedisTopologyMode::Sentinel) => {
            let sentinel = crd
                .topology
                .as_ref()
                .and_then(|t| t.sentinel.as_ref())
                .context("Sentinel mode requires sentinel config")?;

            let hosts: Vec<(String, u16)> = sentinel
                .sentinels
                .iter()
                .filter_map(|s| parse_host_port(s).ok())
                .collect();
            if hosts.is_empty() {
                anyhow::bail!("No valid sentinel endpoints could be parsed");
            }

            Ok(ServerConfig::new_sentinel(hosts, &sentinel.master_name))
        }
        // Standalone (default)
        _ => {
            let (host, port) = parse_redis_url(&endpoints[0])?;
            Ok(ServerConfig::new_centralized(host, port))
        }
    }
}

// ============================================================================
// Builder configuration (pool, reconnect, timeouts, TLS)
// ============================================================================

/// Build a fred `Pool` from CRD config + fred `Config`.
///
/// Applies:
/// - Connection pool size (via `build_pool`)
/// - Reconnection policy (exponential backoff)
/// - Connection timeout + TCP settings
/// - Command timeout (performance config)
/// - TLS (if enabled)
pub fn build_fred_pool(
    crd: &RedisClientConfig,
    config: Config,
) -> Result<(Builder, usize)> {
    let mut builder = Builder::from_config(config);

    // Reconnection policy: exponential backoff, retry forever (max_attempts = 0)
    builder.set_policy(ReconnectPolicy::new_exponential(
        0,      // max_attempts: 0 = infinite
        100,    // min_delay: 100ms
        30_000, // max_delay: 30 seconds
        2,      // base multiplier
    ));

    // Connection config (timeouts, TCP settings)
    let connect_timeout = crd
        .timeout
        .as_ref()
        .and_then(|t| t.connect)
        .map(|ms| ms.min(MAX_CONNECT_TIMEOUT_MS))
        .unwrap_or(DEFAULT_CONNECT_TIMEOUT_MS);

    builder.with_connection_config(|conn| {
        conn.connection_timeout = Duration::from_millis(connect_timeout);
        conn.tcp = TcpConfig {
            nodelay: Some(true),
            ..Default::default()
        };
    });

    // Performance config (command timeout)
    let cmd_timeout = crd
        .timeout
        .as_ref()
        .and_then(|t| t.read.or(t.write))
        .map(|ms| ms.min(MAX_COMMAND_TIMEOUT_MS))
        .unwrap_or(DEFAULT_COMMAND_TIMEOUT_MS);

    builder.with_performance_config(|perf| {
        perf.default_command_timeout = Duration::from_millis(cmd_timeout);
    });

    // TLS configuration
    if let Some(tls) = &crd.tls {
        if tls.enabled {
            // Use default rustls config with system root CA store.
            // TODO: Build rustls ClientConfig from CRD certs (CA, client cert/key)
            // using the same approach as Edgion's core/tls/ module.
            let tls_connector = fred::types::config::TlsConnector::default_rustls()
                .map_err(|e| anyhow::anyhow!("Failed to create rustls TLS config: {:?}", e))?;
            builder.with_config(|config| {
                config.tls = Some(tls_connector.into());
            });

            if tls.insecure_skip_verify.unwrap_or(false) {
                tracing::warn!("Redis TLS insecure_skip_verify is enabled — this should only be used for testing");
            }
        }
    }

    // Connection pool size
    let pool_size = crd
        .pool
        .as_ref()
        .and_then(|p| p.size)
        .map(|s| (s as usize).min(MAX_POOL_SIZE))
        .unwrap_or(DEFAULT_POOL_SIZE);

    Ok((builder, pool_size))
}

// ============================================================================
// URL / address parsing helpers
// ============================================================================

/// Parse "redis://host:port", "rediss://host:port", or "host:port" into (host, port).
pub(crate) fn parse_redis_url(url: &str) -> Result<(String, u16)> {
    // Strip redis:// or rediss:// prefix
    let stripped = url
        .strip_prefix("rediss://")
        .or_else(|| url.strip_prefix("redis://"))
        .unwrap_or(url);

    parse_host_port(stripped)
}

/// Parse "host:port" into (host, port). Defaults to port 6379 if not specified.
pub(crate) fn parse_host_port(addr: &str) -> Result<(String, u16)> {
    // Handle IPv6 addresses like [::1]:6379
    if let Some(bracket_end) = addr.find(']') {
        let host = &addr[..=bracket_end];
        let rest = &addr[bracket_end + 1..];
        if let Some(port_str) = rest.strip_prefix(':') {
            let port: u16 = port_str.parse().context("invalid port in address")?;
            return Ok((host.to_string(), port));
        }
        return Ok((host.to_string(), 6379));
    }

    // Regular host:port
    match addr.rsplit_once(':') {
        Some((host, port_str)) => {
            let port: u16 = port_str.parse().context("invalid port in address")?;
            Ok((host.to_string(), port))
        }
        None => Ok((addr.to_string(), 6379)),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::resources::link_sys::redis::*;

    /// Create a minimal RedisClientConfig for testing
    fn minimal_config(endpoints: Vec<String>) -> RedisClientConfig {
        RedisClientConfig {
            endpoints,
            auth: None,
            db: None,
            timeout: None,
            pool: None,
            retry: None,
            topology: None,
            tls: None,
            observability: None,
        }
    }

    #[test]
    fn test_parse_redis_url_standard() {
        let (host, port) = parse_redis_url("redis://127.0.0.1:6379").unwrap();
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 6379);
    }

    #[test]
    fn test_parse_redis_url_without_scheme() {
        let (host, port) = parse_redis_url("127.0.0.1:6379").unwrap();
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 6379);
    }

    #[test]
    fn test_parse_redis_url_default_port() {
        let (host, port) = parse_redis_url("redis://myhost").unwrap();
        assert_eq!(host, "myhost");
        assert_eq!(port, 6379);
    }

    #[test]
    fn test_parse_redis_url_with_tls() {
        let (host, port) = parse_redis_url("rediss://secure.redis.com:6380").unwrap();
        assert_eq!(host, "secure.redis.com");
        assert_eq!(port, 6380);
    }

    #[test]
    fn test_build_server_config_standalone() {
        let crd = minimal_config(vec!["redis://127.0.0.1:6379".to_string()]);
        let config = build_fred_config(&crd).unwrap();
        assert!(config.server.is_centralized());
    }

    #[test]
    fn test_build_server_config_cluster() {
        let crd = RedisClientConfig {
            endpoints: vec![
                "redis://10.0.0.1:7000".to_string(),
                "redis://10.0.0.2:7001".to_string(),
            ],
            topology: Some(RedisTopology {
                mode: RedisTopologyMode::Cluster,
                sentinel: None,
                cluster: Some(RedisCluster {
                    read_from_replicas: Some(true),
                    max_redirects: Some(5),
                }),
            }),
            ..minimal_config(vec![])
        };
        let config = build_fred_config(&crd).unwrap();
        assert!(config.server.is_clustered());
    }

    #[test]
    fn test_build_server_config_sentinel() {
        let crd = RedisClientConfig {
            endpoints: vec!["redis://sentinel-1:26379".to_string()],
            topology: Some(RedisTopology {
                mode: RedisTopologyMode::Sentinel,
                sentinel: Some(RedisSentinel {
                    master_name: "mymaster".to_string(),
                    sentinels: vec!["sentinel-1:26379".to_string()],
                    password: None,
                }),
                cluster: None,
            }),
            ..minimal_config(vec![])
        };
        let config = build_fred_config(&crd).unwrap();
        assert!(config.server.is_sentinel());
    }

    #[test]
    fn test_empty_endpoints_returns_error() {
        let crd = minimal_config(vec![]);
        assert!(build_fred_config(&crd).is_err());
    }

    #[test]
    fn test_sentinel_without_config_returns_error() {
        let crd = RedisClientConfig {
            endpoints: vec!["redis://sentinel:26379".to_string()],
            topology: Some(RedisTopology {
                mode: RedisTopologyMode::Sentinel,
                sentinel: None,
                cluster: None,
            }),
            ..minimal_config(vec![])
        };
        assert!(build_server_config(&crd).is_err());
    }

    #[test]
    fn test_config_auth_mapping() {
        let crd = RedisClientConfig {
            auth: Some(RedisAuth {
                username: Some("admin".to_string()),
                password: Some("secret".to_string()),
                secret_ref: None,
            }),
            ..minimal_config(vec!["redis://localhost:6379".to_string()])
        };
        let config = build_fred_config(&crd).unwrap();
        assert_eq!(config.username, Some("admin".to_string()));
        assert_eq!(config.password, Some("secret".to_string()));
    }

    #[test]
    fn test_config_db_mapping() {
        let crd = RedisClientConfig {
            db: Some(3),
            ..minimal_config(vec!["redis://localhost:6379".to_string()])
        };
        let config = build_fred_config(&crd).unwrap();
        assert_eq!(config.database, Some(3));
    }

    #[test]
    fn test_pool_size_clamped_to_max() {
        let crd = RedisClientConfig {
            pool: Some(RedisPool {
                size: Some(200),
                min_idle: None,
            }),
            ..minimal_config(vec!["redis://localhost:6379".to_string()])
        };
        let config = build_fred_config(&crd).unwrap();
        let (_builder, pool_size) = build_fred_pool(&crd, config).unwrap();
        assert_eq!(pool_size, MAX_POOL_SIZE);
    }

    #[test]
    fn test_pool_size_default() {
        let crd = minimal_config(vec!["redis://localhost:6379".to_string()]);
        let config = build_fred_config(&crd).unwrap();
        let (_builder, pool_size) = build_fred_pool(&crd, config).unwrap();
        assert_eq!(pool_size, DEFAULT_POOL_SIZE);
    }
}
