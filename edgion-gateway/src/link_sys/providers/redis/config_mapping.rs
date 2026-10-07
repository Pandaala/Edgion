//! CRD `RedisClientConfig` → fred `Config` + `Builder` mapping.
//!
//! This module bridges the CRD configuration types (serde-driven, user-facing YAML)
//! with fred's native configuration structs. The mapping follows the Library-First
//! principle: we use fred's API directly without inventing our own abstractions.

use anyhow::Result;
use fred::types::config::{Config, ReconnectPolicy, ServerConfig, TcpConfig};
use fred::types::Builder;

use crate::link_sys::credential::CredentialState;
use edgion_resources::credentials::materials::BasicAuthMaterial;
use edgion_resources::resources::link_sys::redis::{RedisClientConfig, RedisTopologyMode, REDIS_DEFAULT_POOL_SIZE};

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
///
/// The caller passes the resolved [`CredentialState`] produced by
/// [`credential::basic_state`](crate::link_sys::credential::basic_state).
/// `Pending` / `Invalid` produce a fail-fast `bail!` — dispatch is expected
/// to filter these out up-front by checking
/// `auth_state.is_buildable() && credential::tls_ready(ls)`.
pub fn build_fred_config(crd: &RedisClientConfig, auth: &CredentialState<BasicAuthMaterial>) -> Result<Config> {
    let validation_errors = crd.validation_errors();
    if !validation_errors.is_empty() {
        anyhow::bail!(validation_errors.join("; "));
    }
    let server = build_server_config(crd)?;

    let mut config = Config {
        server,
        ..Default::default()
    };

    if let Some(db) = crd.db {
        config.database = Some(u8::try_from(db).map_err(|_| anyhow::anyhow!("Redis db {} out of range 0-255", db))?);
    }

    if crd.auth.is_some() {
        let m = crate::link_sys::credential::require_basic_material(auth, "Redis")?.ok_or_else(|| {
            anyhow::anyhow!("Redis auth configured in CRD but credential state is NotRequired (internal bug)")
        })?;
        config.username = m.username.clone();
        config.password = Some(m.password.clone());
    }

    Ok(config)
}

/// Map CRD topology to fred `ServerConfig`.
fn build_server_config(crd: &RedisClientConfig) -> Result<ServerConfig> {
    let endpoints = crd
        .parsed_active_endpoints()
        .map_err(|errors| anyhow::anyhow!(errors.join("; ")))?;
    let hosts = endpoints
        .into_iter()
        .map(|endpoint| (endpoint.host, endpoint.port))
        .collect::<Vec<_>>();

    match crd.topology.as_ref().map(|t| &t.mode) {
        // Cluster mode
        Some(RedisTopologyMode::Cluster) => Ok(ServerConfig::new_clustered(hosts)),
        // Sentinel mode
        Some(RedisTopologyMode::Sentinel) => {
            let sentinel = crd
                .topology
                .as_ref()
                .and_then(|t| t.sentinel.as_ref())
                .ok_or_else(|| anyhow::anyhow!("Redis sentinel mode requires topology.sentinel"))?;

            Ok(ServerConfig::new_sentinel(hosts, &sentinel.master_name))
        }
        // Standalone (default)
        _ => {
            let (host, port) = hosts
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("Redis standalone mode requires exactly one endpoint"))?;
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
/// - TLS (when required by a `rediss://` endpoint or explicitly enabled)
pub(super) fn build_fred_pool(
    crd: &RedisClientConfig,
    config: Config,
    connector: Option<fred::types::config::TlsConnector>,
) -> Result<(Builder, usize)> {
    let mut builder = Builder::from_config(config);

    // Reconnect indefinitely so a published client can recover, but do not
    // transparently replay commands whose execution outcome is unknown.
    builder.set_policy(ReconnectPolicy::new_exponential(
        0,      // max_attempts: 0 = infinite
        100,    // min_delay: 100ms
        30_000, // max_delay: 30 seconds
        2,      // base multiplier
    ));

    // Connection config (timeouts, TCP settings)
    let connect_timeout = crd.connect_timeout().map_err(anyhow::Error::msg)?;
    let max_redirections = crd
        .topology
        .as_ref()
        .and_then(|topology| topology.cluster.as_ref())
        .and_then(|cluster| cluster.max_redirects);

    builder.with_connection_config(|conn| {
        conn.connection_timeout = connect_timeout;
        conn.max_command_attempts = 1;
        if let Some(max_redirections) = max_redirections {
            conn.max_redirections = max_redirections;
        }
        conn.tcp = TcpConfig {
            nodelay: Some(true),
            ..Default::default()
        };
    });

    // Performance config (command timeout)
    let command_timeout = crd.command_timeout().map_err(anyhow::Error::msg)?;

    builder.with_performance_config(|perf| {
        perf.default_command_timeout = command_timeout;
    });

    if let Some(tls_connector) = connector {
        builder.with_config(|config| {
            config.tls = Some(tls_connector.into());
        });
    }

    let pool_size = crd
        .pool
        .as_ref()
        .and_then(|p| p.size)
        .unwrap_or(REDIS_DEFAULT_POOL_SIZE) as usize;

    Ok((builder, pool_size))
}

// ============================================================================
// Tests
// ============================================================================
#[cfg(test)]
#[path = "../../../../tests/unit/link_sys/providers/redis/config_mapping_test.rs"]
mod tests;
