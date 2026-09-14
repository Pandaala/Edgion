//! CRD `RedisClientConfig` → fred `Config` + `Builder` mapping.
//!
//! This module bridges the CRD configuration types (serde-driven, user-facing YAML)
//! with fred's native configuration structs. The mapping follows the Library-First
//! principle: we use fred's API directly without inventing our own abstractions.

use anyhow::{Context, Result};
use fred::types::config::{Config, ReconnectPolicy, ServerConfig, TcpConfig};
use fred::types::Builder;

use crate::link_sys::credential::CredentialState;
use edgion_resources::credentials::materials::BasicAuthMaterial;
use edgion_resources::resources::common::TlsConfig;
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
    let endpoints = &crd.endpoints;
    if endpoints.is_empty() {
        anyhow::bail!("Redis endpoints list is empty");
    }

    match crd.topology.as_ref().map(|t| &t.mode) {
        // Cluster mode
        Some(RedisTopologyMode::Cluster) => {
            let hosts: Vec<(String, u16)> = endpoints.iter().filter_map(|ep| parse_redis_url(ep).ok()).collect();
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
                .filter_map(|s| parse_redis_url(s).ok())
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
/// - TLS (when required by a `rediss://` endpoint or explicitly enabled)
pub fn build_fred_pool(crd: &RedisClientConfig, config: Config) -> Result<(Builder, usize)> {
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

    let explicit_tls = crd.tls.as_ref().is_some_and(|tls| tls.enabled);
    let scheme_requires_tls = if explicit_tls {
        false
    } else {
        implicit_tls_required(crd)?
    };
    if scheme_requires_tls || explicit_tls {
        let default_tls = TlsConfig::default();
        // A disabled TLS block cannot customize an implicitly secure rediss://
        // transport. Only an explicitly enabled block contributes CA, mTLS,
        // and verification policy; implicit TLS uses the library defaults.
        let tls = crd.tls.as_ref().filter(|tls| tls.enabled).unwrap_or(&default_tls);

        // fred's rustls connector always verifies; reject verify=false up front
        // (provider_can_disable=false → rejected even outside production).
        crate::link_sys::outbound_tls::resolve_outbound_tls(
            &tls.outbound,
            tls.resolved_ca_certificates.as_deref(),
            tls.resolved_client_certificate.as_ref(),
            false,
            "Redis",
        )
        .map_err(|e| {
            anyhow::anyhow!(
                "Redis TLS configuration error: {e:?}. fred does not support skip-verify; \
                 use a proper CA certificate or connect without TLS."
            )
        })?;
        // Prefer a custom rustls `ClientConfig` built from CRD / Secret
        // material (private CA, client mTLS). Falls back to fred's default
        // rustls connector (system CA roots, no client auth) when the CRD
        // carries no TLS material at all.
        let tls_connector = match crate::link_sys::tls::build_rustls_client_config(tls)? {
            Some(client_config) => fred::types::config::TlsConnector::from(client_config),
            None => fred::types::config::TlsConnector::default_rustls()
                .map_err(|e| anyhow::anyhow!("Failed to create default rustls TLS config: {:?}", e))?,
        };
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
// URL / address parsing helpers
// ============================================================================

/// Return the endpoints used by the selected topology.
///
/// Sentinel uses its dedicated endpoint list, cluster uses every top-level
/// endpoint, and standalone uses only its first top-level endpoint. This keeps
/// URL parsing and transport selection aligned with [`build_server_config`].
fn active_endpoints(crd: &RedisClientConfig) -> impl Iterator<Item = &str> {
    let endpoints = match crd.topology.as_ref().map(|topology| &topology.mode) {
        Some(RedisTopologyMode::Cluster) => crd.endpoints.as_slice(),
        Some(RedisTopologyMode::Sentinel) => crd
            .topology
            .as_ref()
            .and_then(|topology| topology.sentinel.as_ref())
            .map(|sentinel| sentinel.sentinels.as_slice())
            .unwrap_or_default(),
        _ => crd.endpoints.get(..1).unwrap_or_default(),
    };

    endpoints.iter().map(String::as_str)
}

/// Determine whether endpoint schemes require implicit TLS.
///
/// fred applies one TLS connector to the whole client configuration, so a
/// topology cannot safely combine secure and plaintext active endpoints unless
/// the operator explicitly enables TLS for the complete topology.
fn implicit_tls_required(crd: &RedisClientConfig) -> Result<bool> {
    let mut has_secure = false;
    let mut has_plaintext = false;

    for endpoint in active_endpoints(crd) {
        if endpoint.starts_with("rediss://") {
            has_secure = true;
        } else {
            has_plaintext = true;
        }
    }

    if has_secure && has_plaintext {
        anyhow::bail!(
            "Redis active endpoints cannot mix TLS and plaintext transports; use consistent schemes or set tls.enabled=true"
        );
    }

    Ok(has_secure)
}

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
    use crate::link_sys::credential::{CredentialState, InvalidReason};
    use edgion_resources::resources::link_sys::redis::*;

    /// Create a minimal RedisClientConfig for testing
    fn minimal_config(endpoints: Vec<String>) -> RedisClientConfig {
        RedisClientConfig {
            endpoints,
            auth: None,
            db: None,
            timeout: None,
            pool: None,
            topology: None,
            tls: None,
        }
    }

    /// Shorthand: `NotRequired` state (no auth configured).
    fn no_auth() -> CredentialState<BasicAuthMaterial> {
        CredentialState::NotRequired
    }

    fn uses_tls(crd: &RedisClientConfig) -> Result<bool> {
        let config = build_fred_config(crd, &no_auth())?;
        let (builder, _) = build_fred_pool(crd, config)?;
        Ok(builder.get_config().is_some_and(Config::uses_tls))
    }

    fn assert_outbound_tls_in_subprocess(test_name: &str, assertion: impl FnOnce()) {
        const CHILD_ENV: &str = "EDGION_REDIS_TLS_MAPPING_CHILD";
        const TEST_MODULE: &str = "link_sys::providers::redis::config_mapping::tests";

        if std::env::var(CHILD_ENV).as_deref() == Ok(test_name) {
            // This process runs only the exact test selected below, so resolving
            // the process-global outbound TLS default cannot race another test.
            let _ = rustls::crypto::ring::default_provider().install_default();
            assertion();
            return;
        }

        let exact_test_name = format!("{TEST_MODULE}::{test_name}");
        let output = std::process::Command::new(
            std::env::current_exe().expect("the Rust test harness must have a current executable"),
        )
        .args(["--exact", exact_test_name.as_str(), "--nocapture"])
        .env(CHILD_ENV, test_name)
        .output()
        .expect("the isolated Redis TLS mapping test process must start");
        assert!(
            output.status.success(),
            "isolated Redis TLS mapping test failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
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
        let config = build_fred_config(&crd, &no_auth()).unwrap();
        assert!(config.server.is_centralized());
    }

    #[test]
    fn test_build_server_config_cluster() {
        let crd = RedisClientConfig {
            endpoints: vec!["redis://10.0.0.1:7000".to_string(), "redis://10.0.0.2:7001".to_string()],
            topology: Some(RedisTopology {
                mode: RedisTopologyMode::Cluster,
                sentinel: None,
                cluster: Some(RedisCluster { max_redirects: Some(5) }),
            }),
            ..minimal_config(vec![])
        };
        let config = build_fred_config(&crd, &no_auth()).unwrap();
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
                }),
                cluster: None,
            }),
            ..minimal_config(vec![])
        };
        let config = build_fred_config(&crd, &no_auth()).unwrap();
        assert!(config.server.is_sentinel());
    }

    #[test]
    fn sentinel_endpoints_accept_redis_schemes() {
        assert_outbound_tls_in_subprocess("sentinel_endpoints_accept_redis_schemes", || {
            let crd = RedisClientConfig {
                endpoints: vec!["redis://unused:6379".to_string()],
                topology: Some(RedisTopology {
                    mode: RedisTopologyMode::Sentinel,
                    sentinel: Some(RedisSentinel {
                        master_name: "mymaster".to_string(),
                        sentinels: vec!["rediss://sentinel-1:26379".to_string()],
                    }),
                    cluster: None,
                }),
                ..minimal_config(vec![])
            };

            let config = build_fred_config(&crd, &no_auth()).unwrap();
            let ServerConfig::Sentinel { hosts, .. } = config.server else {
                panic!("expected sentinel server config");
            };
            assert_eq!(hosts[0].host, "sentinel-1");
            assert_eq!(hosts[0].port, 26379);
            assert!(uses_tls(&crd).unwrap());
        });
    }

    #[test]
    fn test_empty_endpoints_returns_error() {
        let crd = minimal_config(vec![]);
        assert!(build_fred_config(&crd, &no_auth()).is_err());
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

    fn crd_with_secret_ref() -> RedisClientConfig {
        use edgion_resources::resources::common::{SecretObjectReference, SecretSlot};
        RedisClientConfig {
            auth: Some(RedisAuth {
                slot: SecretSlot {
                    secret_ref: SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "redis-creds".to_string(),
                        namespace: None,
                    },
                    secret: None,
                },
            }),
            ..minimal_config(vec!["redis://localhost:6379".to_string()])
        }
    }

    #[test]
    fn test_config_auth_mapping() {
        let crd = crd_with_secret_ref();
        let state = CredentialState::Resolved(BasicAuthMaterial {
            username: Some("u".to_string()),
            password: "p".to_string(),
        });
        let config = build_fred_config(&crd, &state).unwrap();
        assert_eq!(config.username, Some("u".to_string()));
        assert_eq!(config.password, Some("p".to_string()));
    }

    /// Regression guard: auth configured in CRD but state is `Pending` must
    /// fail-fast instead of silently falling back to empty credentials.
    #[test]
    fn test_config_auth_pending_fails_fast() {
        let crd = crd_with_secret_ref();
        let err = build_fred_config(&crd, &CredentialState::Pending).expect_err("should refuse pending");
        assert!(
            err.to_string().contains("has not been resolved"),
            "unexpected error message: {err}"
        );
    }

    /// Regression guard: auth configured in CRD, state is `Invalid` → fail-fast
    /// with a distinct message carrying the reason.
    #[test]
    fn test_config_auth_invalid_fails_fast() {
        let crd = crd_with_secret_ref();
        let state = CredentialState::Invalid(InvalidReason::ParseFailed("missing keys: [password]".into()));
        let err = build_fred_config(&crd, &state).expect_err("should refuse invalid");
        assert!(
            err.to_string().contains("invalid") && err.to_string().contains("missing keys"),
            "unexpected error message: {err}"
        );
    }

    /// Invariant guard: the resolver-selection logic in
    /// `credential::basic_state` must never return `NotRequired` when the CRD
    /// declares `auth`. If that invariant is ever broken (e.g. future
    /// refactor of resolver dispatch), this test locks the fail-fast arm in
    /// `build_fred_config` so CI catches it instead of silently authenticating
    /// anonymously.
    #[test]
    fn test_config_crd_has_auth_but_state_not_required_is_internal_bug() {
        let crd = crd_with_secret_ref();
        let err = build_fred_config(&crd, &CredentialState::NotRequired).expect_err("should refuse");
        assert!(
            err.to_string().contains("internal bug"),
            "unexpected error message: {err}"
        );
    }

    /// Password-only basic auth (username = None) is a legitimate configuration
    /// supported by `BasicAuthResolver`; ensure we don't regress into rejecting
    /// this valid case.
    #[test]
    fn test_config_auth_password_only_material_is_accepted() {
        let crd = crd_with_secret_ref();
        let state = CredentialState::Resolved(BasicAuthMaterial {
            username: None,
            password: "pw".to_string(),
        });
        let config = build_fred_config(&crd, &state).unwrap();
        assert_eq!(config.username, None);
        assert_eq!(config.password, Some("pw".to_string()));
    }

    #[test]
    fn test_config_db_mapping() {
        let crd = RedisClientConfig {
            db: Some(3),
            ..minimal_config(vec!["redis://localhost:6379".to_string()])
        };
        let config = build_fred_config(&crd, &no_auth()).unwrap();
        assert_eq!(config.database, Some(3));
    }

    #[test]
    fn pool_size_above_max_is_rejected() {
        let crd = RedisClientConfig {
            pool: Some(RedisPool { size: Some(200) }),
            ..minimal_config(vec!["redis://localhost:6379".to_string()])
        };
        let error = build_fred_config(&crd, &no_auth()).unwrap_err();
        assert!(error.to_string().contains("pool.size must be between 1 and 64"));
    }

    #[test]
    fn test_pool_size_default() {
        let crd = minimal_config(vec!["redis://localhost:6379".to_string()]);
        let config = build_fred_config(&crd, &no_auth()).unwrap();
        let (_builder, pool_size) = build_fred_pool(&crd, config).unwrap();
        assert_eq!(pool_size, REDIS_DEFAULT_POOL_SIZE as usize);
    }

    #[test]
    fn rediss_without_tls_block_enables_tls() {
        assert_outbound_tls_in_subprocess("rediss_without_tls_block_enables_tls", || {
            let crd = minimal_config(vec!["rediss://redis.example.com:6380".to_string()]);

            assert!(uses_tls(&crd).unwrap());
        });
    }

    #[test]
    fn rediss_with_disabled_tls_block_still_enables_tls() {
        assert_outbound_tls_in_subprocess("rediss_with_disabled_tls_block_still_enables_tls", || {
            use edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef;
            use edgion_resources::resources::common::{OutboundTlsLocal, OutboundTlsValidation, SecretObjectReference};
            use k8s_openapi::api::core::v1::Secret;

            let crd = RedisClientConfig {
                tls: Some(TlsConfig {
                    enabled: false,
                    outbound: OutboundTlsLocal {
                        validation: Some(OutboundTlsValidation {
                            ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
                                group: String::new(),
                                kind: "Secret".to_string(),
                                name: "ignored-ca".to_string(),
                                namespace: None,
                            }]),
                            ..Default::default()
                        }),
                        client_certificate_ref: Some(SecretObjectReference {
                            group: None,
                            kind: None,
                            name: "ignored-client".to_string(),
                            namespace: None,
                        }),
                        ..Default::default()
                    },
                    // Invalid placeholder material would fail closed if the
                    // disabled policy reached the shared TLS resolver.
                    resolved_ca_certificates: Some(vec![Secret::default()]),
                    resolved_client_certificate: Some(Secret::default()),
                }),
                ..minimal_config(vec!["rediss://redis.example.com:6380".to_string()])
            };

            assert!(uses_tls(&crd).unwrap());
        });
    }

    #[test]
    fn redis_without_enabled_tls_remains_plaintext() {
        let crd = minimal_config(vec!["redis://redis.example.com:6379".to_string()]);

        assert!(!uses_tls(&crd).unwrap());
    }

    #[test]
    fn standalone_ignores_later_rediss_endpoint() {
        assert_outbound_tls_in_subprocess("standalone_ignores_later_rediss_endpoint", || {
            let crd = minimal_config(vec![
                "redis://redis.example.com:6379".to_string(),
                "rediss://unused.example.com:6380".to_string(),
            ]);

            assert!(!uses_tls(&crd).unwrap());
        });
    }

    #[test]
    fn redis_with_explicit_tls_enables_tls() {
        assert_outbound_tls_in_subprocess("redis_with_explicit_tls_enables_tls", || {
            let crd = RedisClientConfig {
                tls: Some(TlsConfig {
                    enabled: true,
                    ..Default::default()
                }),
                ..minimal_config(vec!["redis://redis.example.com:6379".to_string()])
            };

            assert!(uses_tls(&crd).unwrap());
        });
    }

    #[test]
    fn rediss_cluster_endpoints_enable_tls() {
        assert_outbound_tls_in_subprocess("rediss_cluster_endpoints_enable_tls", || {
            let crd = RedisClientConfig {
                endpoints: vec![
                    "rediss://redis-1.example.com:6380".to_string(),
                    "rediss://redis-2.example.com:6380".to_string(),
                ],
                topology: Some(RedisTopology {
                    mode: RedisTopologyMode::Cluster,
                    sentinel: None,
                    cluster: Some(RedisCluster { max_redirects: Some(5) }),
                }),
                ..minimal_config(vec![])
            };

            assert!(uses_tls(&crd).unwrap());
        });
    }

    #[test]
    fn mixed_cluster_endpoint_transports_are_rejected_without_explicit_tls() {
        let crd = RedisClientConfig {
            endpoints: vec![
                "rediss://redis-1.example.com:6380".to_string(),
                "redis-2.example.com:6379".to_string(),
            ],
            topology: Some(RedisTopology {
                mode: RedisTopologyMode::Cluster,
                sentinel: None,
                cluster: Some(RedisCluster { max_redirects: Some(5) }),
            }),
            ..minimal_config(vec![])
        };

        let config = build_fred_config(&crd, &no_auth()).unwrap();
        let err = build_fred_pool(&crd, config).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Redis active endpoints cannot mix TLS and plaintext transports; use consistent schemes or set tls.enabled=true"
        );
    }

    #[test]
    fn mixed_sentinel_endpoint_transports_are_rejected_without_explicit_tls() {
        let crd = RedisClientConfig {
            endpoints: vec!["redis://unused:6379".to_string()],
            topology: Some(RedisTopology {
                mode: RedisTopologyMode::Sentinel,
                sentinel: Some(RedisSentinel {
                    master_name: "mymaster".to_string(),
                    sentinels: vec![
                        "rediss://sentinel-1:26380".to_string(),
                        "redis://sentinel-2:26379".to_string(),
                    ],
                }),
                cluster: None,
            }),
            ..minimal_config(vec![])
        };

        let config = build_fred_config(&crd, &no_auth()).unwrap();
        let err = build_fred_pool(&crd, config).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Redis active endpoints cannot mix TLS and plaintext transports; use consistent schemes or set tls.enabled=true"
        );
    }

    #[test]
    fn explicit_tls_allows_mixed_cluster_endpoint_spelling() {
        assert_outbound_tls_in_subprocess("explicit_tls_allows_mixed_cluster_endpoint_spelling", || {
            let crd = RedisClientConfig {
                endpoints: vec![
                    "rediss://redis-1.example.com:6380".to_string(),
                    "redis://redis-2.example.com:6379".to_string(),
                ],
                topology: Some(RedisTopology {
                    mode: RedisTopologyMode::Cluster,
                    sentinel: None,
                    cluster: Some(RedisCluster { max_redirects: Some(5) }),
                }),
                tls: Some(TlsConfig {
                    enabled: true,
                    ..Default::default()
                }),
                ..minimal_config(vec![])
            };

            assert!(uses_tls(&crd).unwrap());
        });
    }

    #[test]
    fn command_contract_maps_timeouts_retries_and_redirects() {
        let crd = RedisClientConfig {
            timeout: Some(RedisTimeout {
                connect: Some("3s".to_string()),
                command: Some("2s".to_string()),
            }),
            topology: Some(RedisTopology {
                mode: RedisTopologyMode::Cluster,
                sentinel: None,
                cluster: Some(RedisCluster { max_redirects: Some(6) }),
            }),
            ..minimal_config(vec!["redis://redis.example.com:6379".to_string()])
        };
        let config = build_fred_config(&crd, &no_auth()).unwrap();
        let (builder, _) = build_fred_pool(&crd, config).unwrap();
        let connection = builder.get_connection_config();
        let performance = builder.get_performance_config();

        assert_eq!(connection.connection_timeout, std::time::Duration::from_secs(3));
        assert_eq!(connection.max_command_attempts, 1);
        assert_eq!(connection.max_redirections, 6);
        assert_eq!(performance.default_command_timeout, std::time::Duration::from_secs(2));
    }

    /// Guard: fred's rustls connector always verifies — verify=false must be
    /// rejected regardless of environment (provider_can_disable=false).
    #[test]
    fn redis_rejects_verify_false_always() {
        use edgion_resources::resources::common::{OutboundTlsLocal, TlsConfig};

        assert_outbound_tls_in_subprocess("redis_rejects_verify_false_always", || {
            let crd = RedisClientConfig {
                tls: Some(TlsConfig {
                    enabled: true,
                    outbound: OutboundTlsLocal {
                        verify: Some(false),
                        ..Default::default()
                    },
                    resolved_ca_certificates: None,
                    resolved_client_certificate: None,
                }),
                ..minimal_config(vec!["redis://localhost:6379".to_string()])
            };
            // provider_can_disable=false causes the resolver to reject verify=false
            // (Redis SDK has no skip-verify API) regardless of any environment variable.
            let err = build_fred_pool(&crd, build_fred_config(&crd, &no_auth()).unwrap()).unwrap_err();
            assert!(
                err.to_string().contains("Redis TLS configuration error"),
                "unexpected error: {err}"
            );
        });
    }
}
