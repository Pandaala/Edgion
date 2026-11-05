use super::*;
use crate::link_sys::credential::{CredentialState, InvalidReason};
use edgion_common::link_sys_core::outbound_tls::PreparedRedisTls;
use edgion_resources::credentials::materials::BasicAuthMaterial;
use edgion_resources::resources::common::TlsConfig;
use edgion_resources::resources::link_sys::redis::*;
use edgion_resources::resources::link_sys::{LinkSys, LinkSysSpec, SystemConfig};
use std::sync::Arc;

fn build_transport(crd: &RedisClientConfig, auth: &CredentialState<BasicAuthMaterial>) -> anyhow::Result<Transport> {
    let errors = crd.validation_errors();
    anyhow::ensure!(errors.is_empty(), "{}", errors.join("; "));
    let mut owner = LinkSys::new(
        "redis-mapping",
        LinkSysSpec {
            config: SystemConfig::Redis(crd.clone()),
            current_status: None,
            unknown_fields: Vec::new(),
        },
    );
    owner.metadata.namespace = Some("mapping".into());
    let prepared = PreparedRedisTls::from_link_sys(Arc::new(owner))?;
    Transport::new(crd, auth, prepared.redis_tls_material()?)
}

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

fn uses_tls(crd: &RedisClientConfig) -> anyhow::Result<bool> {
    Ok(build_transport(crd, &no_auth())?.tls.is_some())
}

fn assert_outbound_tls_in_subprocess(test_name: &str, assertion: impl FnOnce()) {
    const CHILD_ENV: &str = "EDGION_REDIS_TLS_MAPPING_CHILD";
    const TEST_MODULE: &str = "link_sys::providers::redis::transport::tests";

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
fn test_build_server_config_standalone() {
    let crd = minimal_config(vec!["redis://127.0.0.1:6379".to_string()]);
    let config = build_transport(&crd, &no_auth()).unwrap();
    assert!(matches!(config.mode, RedisTopologyMode::Standalone));
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
    let config = build_transport(&crd, &no_auth()).unwrap();
    assert!(matches!(config.mode, RedisTopologyMode::Cluster));
}

#[test]
fn cluster_rejects_the_complete_config_when_one_endpoint_is_invalid() {
    let crd = RedisClientConfig {
        endpoints: vec![
            "redis://10.0.0.1:7000".to_string(),
            "redis://user:password@10.0.0.2:7001".to_string(),
        ],
        topology: Some(RedisTopology {
            mode: RedisTopologyMode::Cluster,
            sentinel: None,
            cluster: None,
        }),
        ..minimal_config(vec![])
    };

    let error = build_transport(&crd, &no_auth()).err().unwrap().to_string();
    assert!(error.contains("Redis endpoints[1] is invalid"));
    assert!(!error.contains("password"));
}

#[test]
fn test_build_server_config_sentinel() {
    let crd = RedisClientConfig {
        endpoints: Vec::new(),
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
    let config = build_transport(&crd, &no_auth()).unwrap();
    assert!(matches!(config.mode, RedisTopologyMode::Sentinel));
}

#[test]
fn sentinel_endpoints_accept_redis_schemes() {
    assert_outbound_tls_in_subprocess("sentinel_endpoints_accept_redis_schemes", || {
        let crd = RedisClientConfig {
            endpoints: Vec::new(),
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

        let config = build_transport(&crd, &no_auth()).unwrap();
        assert_eq!(config.endpoints[0], ("sentinel-1".into(), 26379));
        assert!(uses_tls(&crd).unwrap());
    });
}

#[test]
fn test_empty_endpoints_returns_error() {
    let crd = minimal_config(vec![]);
    assert!(build_transport(&crd, &no_auth()).is_err());
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
    assert!(build_transport(&crd, &no_auth()).is_err());
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
    let config = build_transport(&crd, &state).unwrap();
    assert_eq!(config.settings.username(), Some("u"));
    assert_eq!(config.settings.password(), Some("p"));
}

/// Regression guard: auth configured in CRD but state is `Pending` must
/// fail-fast instead of silently falling back to empty credentials.
#[test]
fn test_config_auth_pending_fails_fast() {
    let crd = crd_with_secret_ref();
    let err = build_transport(&crd, &CredentialState::Pending)
        .err()
        .expect("should refuse pending");
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
    let err = build_transport(&crd, &state).err().expect("should refuse invalid");
    assert!(
        err.to_string().contains("invalid") && err.to_string().contains("missing keys"),
        "unexpected error message: {err}"
    );
}

/// Invariant guard: the resolver-selection logic in
/// `credential::basic_state` must never return `NotRequired` when the CRD
/// declares `auth`. If that invariant is ever broken (e.g. future
/// refactor of resolver dispatch), this test locks the fail-fast arm in
/// `build_transport` so CI catches it instead of silently authenticating
/// anonymously.
#[test]
fn test_config_crd_has_auth_but_state_not_required_is_internal_bug() {
    let crd = crd_with_secret_ref();
    let err = build_transport(&crd, &CredentialState::NotRequired)
        .err()
        .expect("should refuse");
    assert!(
        err.to_string().contains("credentials are unavailable"),
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
    let config = build_transport(&crd, &state).unwrap();
    assert_eq!(config.settings.username(), None);
    assert_eq!(config.settings.password(), Some("pw"));
}

#[test]
fn test_config_db_mapping() {
    let crd = RedisClientConfig {
        db: Some(3),
        ..minimal_config(vec!["redis://localhost:6379".to_string()])
    };
    let config = build_transport(&crd, &no_auth()).unwrap();
    assert_eq!(config.settings.db(), 3);
}

#[test]
fn pool_size_above_max_is_rejected() {
    let crd = RedisClientConfig {
        pool: Some(RedisPool { size: Some(200) }),
        ..minimal_config(vec!["redis://localhost:6379".to_string()])
    };
    let error = build_transport(&crd, &no_auth()).err().unwrap();
    assert!(error.to_string().contains("pool.size must be between 1 and 64"));
}

#[test]
fn test_pool_size_default() {
    let crd = minimal_config(vec!["redis://localhost:6379".to_string()]);
    let config = build_transport(&crd, &no_auth()).unwrap();
    let pool_size = config.pool_size;
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
        use edgion_resources::resources::common::CustomCACertificateRef;
        use edgion_resources::resources::common::{OutboundTlsLocal, OutboundTlsValidation, SecretObjectReference};
        use k8s_openapi::api::core::v1::Secret;

        let crd = RedisClientConfig {
            tls: Some(TlsConfig {
                enabled: false,
                outbound: OutboundTlsLocal {
                    validation: Some(OutboundTlsValidation {
                        ca_certificate_refs: Some(vec![CustomCACertificateRef {
                            group: Some(String::new()),
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
fn standalone_rejects_extra_endpoints() {
    let crd = minimal_config(vec![
        "redis://redis.example.com:6379".to_string(),
        "rediss://unused.example.com:6380".to_string(),
    ]);

    let error = build_transport(&crd, &no_auth()).err().unwrap();
    assert!(error
        .to_string()
        .contains("Redis standalone mode requires exactly one endpoint"));
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

    let err = build_transport(&crd, &no_auth()).err().unwrap();
    assert_eq!(
        err.to_string(),
        "Redis active endpoints cannot mix TLS and plaintext transports unless tls.enabled=true"
    );
}

#[test]
fn mixed_sentinel_endpoint_transports_are_rejected_without_explicit_tls() {
    let crd = RedisClientConfig {
        endpoints: Vec::new(),
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

    let err = build_transport(&crd, &no_auth()).err().unwrap();
    assert_eq!(
        err.to_string(),
        "Redis active endpoints cannot mix TLS and plaintext transports unless tls.enabled=true"
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
    let config = build_transport(&crd, &no_auth()).unwrap();
    assert_eq!(config.connect_timeout, std::time::Duration::from_secs(3));
    assert_eq!(config.command_timeout, std::time::Duration::from_secs(2));
    assert_eq!(config.max_redirects, 6);
}

/// Guard: redis-rs's rustls connector always verifies — verify=false must be
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
        let err = build_transport(&crd, &no_auth()).err().unwrap();
        assert!(
            err.to_string().contains("Redis TLS configuration error"),
            "unexpected error: {err}"
        );
    });
}
