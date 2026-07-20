//! Etcd ConfCenter configuration schema + client-config conversion.
//!
//! YAML-friendly (snake_case) configuration for the etcd backend, plus the
//! conversion to `EtcdClientConfig` consumed by `EtcdLinkClient::from_config()`.
//! This schema lives in the controller (`crate::config::conf_center::etcd`); the
//! etcd link client and credential-state types it depends on live in `edgion-common`.

use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use edgion_common::link_sys_core::credential::CredentialState;
use edgion_resources::credentials::materials::BasicAuthMaterial;
use edgion_resources::credentials::resolvers::tls::{TLS_CA_KEY, TLS_CERT_KEY, TLS_KEY_KEY};
use edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef;
use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
use edgion_resources::resources::common::{OutboundTlsLocal, SecretObjectReference, SecretSlot, TlsConfig};
use edgion_resources::resources::link_sys::etcd::{EtcdAuth, EtcdClientConfig, EtcdKeepAlive, EtcdTimeout};

// ============================================================================
// EtcdConfig — YAML top-level config
// ============================================================================

/// Etcd configuration center settings
///
/// ## Example (YAML)
///
/// ```yaml
/// conf_center:
///   type: etcd
///   endpoints: ["http://127.0.0.1:2379"]
///   key_prefix: /edgion
///   auth:
///     username: root
///     password: changeme
///   tls:
///     ca_cert_file: /etc/edgion/certs/etcd-ca.pem
///     client_cert_file: /etc/edgion/certs/etcd-client.pem
///     client_key_file: /etc/edgion/certs/etcd-client-key.pem
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EtcdConfig {
    /// etcd cluster endpoint list (e.g., ["http://127.0.0.1:2379"])
    pub endpoints: Vec<String>,

    /// Key prefix for all configuration keys (default: "/edgion")
    #[serde(default = "default_key_prefix")]
    pub key_prefix: String,

    /// Authentication (username/password)
    #[serde(default)]
    pub auth: Option<EtcdAuthConfig>,

    /// TLS configuration (file paths for certificates)
    #[serde(default)]
    pub tls: Option<EtcdTlsConfig>,

    /// Connection (dial) timeout in milliseconds (default: 5000)
    #[serde(default)]
    pub connect_timeout_ms: Option<u64>,

    /// Per-request timeout in milliseconds (default: 10000)
    #[serde(default)]
    pub request_timeout_ms: Option<u64>,

    /// HTTP/2 keep-alive interval in seconds
    #[serde(default)]
    pub keep_alive_time_secs: Option<u64>,

    /// HTTP/2 keep-alive timeout in seconds
    #[serde(default)]
    pub keep_alive_timeout_secs: Option<u64>,

    /// Leader election (etcd HA). Always on in etcd mode (spec §8).
    #[serde(default)]
    pub leader_election: EtcdLeaderElectionConfig,
}

pub(crate) fn default_key_prefix() -> String {
    "/edgion".to_string()
}

fn default_lease_duration_secs() -> u64 {
    15
}
fn default_renew_deadline_secs() -> u64 {
    10
}
fn default_retry_period_secs() -> u64 {
    2
}

/// Etcd-mode leader election settings. Always on; there is no enable switch
/// (spec §8). `identity` is human-facing only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EtcdLeaderElectionConfig {
    /// Human-readable identity for logs and the not-leader 409 body. Never used
    /// for an ownership decision. Defaults to the hostname.
    #[serde(default)]
    pub identity: Option<String>,
    /// conf_sync gRPC address gateways rotate to. Required; must be routable.
    pub grpc_addr: String,
    /// Admin HTTP address the not-leader 409 body names. Required; must be routable.
    pub admin_addr: String,
    #[serde(default = "default_lease_duration_secs")]
    pub lease_duration_secs: u64,
    #[serde(default = "default_renew_deadline_secs")]
    pub renew_deadline_secs: u64,
    #[serde(default = "default_retry_period_secs")]
    pub retry_period_secs: u64,
}

impl Default for EtcdLeaderElectionConfig {
    fn default() -> Self {
        Self {
            identity: None,
            grpc_addr: String::new(),
            admin_addr: String::new(),
            lease_duration_secs: default_lease_duration_secs(),
            renew_deadline_secs: default_renew_deadline_secs(),
            retry_period_secs: default_retry_period_secs(),
        }
    }
}

impl EtcdLeaderElectionConfig {
    /// Resolve the human-facing identity: explicit `identity`, else the
    /// `HOSTNAME` environment variable, else a fixed fallback. Identity is never
    /// used for an ownership decision (spec §8).
    pub fn resolved_identity(&self) -> String {
        if let Some(id) = self.identity.as_ref() {
            if !id.is_empty() {
                return id.clone();
            }
        }
        match std::env::var("HOSTNAME") {
            Ok(h) if !h.is_empty() => h,
            _ => "edgion-controller".to_string(),
        }
    }

    fn addr_is_routable(addr: &str) -> bool {
        if addr.is_empty() {
            return false;
        }
        // Reject loopback / unspecified host portions. A leader advertising
        // 127.0.0.1 or 0.0.0.0 yields a 409 body / rotation target that can
        // never work (spec §8).
        let host = addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(addr);
        let host = host.trim_start_matches('[').trim_end_matches(']');
        !(host == "127.0.0.1" || host == "0.0.0.0" || host == "::1" || host == "::" || host == "localhost")
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        let (r, d, l) = (
            self.retry_period_secs,
            self.renew_deadline_secs,
            self.lease_duration_secs,
        );
        if r == 0 || d == 0 || l == 0 {
            anyhow::bail!("etcd leader_election: all timing values must be > 0 (retry={r}, renew={d}, lease={l})");
        }
        if !(r < d && d < l) {
            anyhow::bail!(
                "etcd leader_election: require retry_period < renew_deadline < lease_duration \
                 (got {r} < {d} < {l})"
            );
        }
        if d / r < 3 {
            anyhow::bail!(
                "etcd leader_election: floor(renew_deadline/retry_period) must be >= 3 for enough \
                 keepalive attempts (got floor({d}/{r})={})",
                d / r
            );
        }
        if !Self::addr_is_routable(&self.grpc_addr) {
            anyhow::bail!(
                "etcd leader_election: grpc_addr must be a routable non-loopback address (got {:?})",
                self.grpc_addr
            );
        }
        if !Self::addr_is_routable(&self.admin_addr) {
            anyhow::bail!(
                "etcd leader_election: admin_addr must be a routable non-loopback address (got {:?})",
                self.admin_addr
            );
        }
        Ok(())
    }
}

/// Authentication configuration for etcd (YAML snake_case)
#[derive(Clone, Serialize, Deserialize)]
pub struct EtcdAuthConfig {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for EtcdAuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EtcdAuthConfig")
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

/// TLS configuration for etcd (YAML snake_case, file paths)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EtcdTlsConfig {
    /// CA certificate file path (PEM)
    #[serde(default)]
    pub ca_cert_file: Option<String>,
    /// Client certificate file path (PEM) — for mTLS
    #[serde(default)]
    pub client_cert_file: Option<String>,
    /// Client private key file path (PEM) — for mTLS
    #[serde(default)]
    pub client_key_file: Option<String>,
}

// ============================================================================
// Conversion to EtcdClientConfig (for EtcdLinkClient::from_config)
// ============================================================================

impl EtcdConfig {
    /// Convert to `EtcdClientConfig` + `CredentialState` that
    /// `EtcdLinkClient::from_config()` accepts.
    ///
    /// TLS certificate file paths are read from disk and embedded as PEM strings.
    ///
    /// Auth note: Controller's ConfCenter uses file-based credentials (YAML
    /// `conf_center.auth`), not `SecretObjectReference`. To reuse the same
    /// `build_connect_options` code path as the LinkSys CRD flow, we:
    /// - fill `EtcdClientConfig.auth` with a synthetic `EtcdAuth` placeholder
    ///   (the `secret_ref` is never read by the Gateway provider; only
    ///   `crd.auth.is_some()` is inspected as the "auth configured" gate);
    /// - return a [`CredentialState::Resolved`] carrying the YAML material so
    ///   the provider's auth arm gets the plaintext `username` / `password`.
    ///
    /// If no `conf_center.auth` section is configured, returns
    /// `(EtcdClientConfig { auth: None, .. }, CredentialState::NotRequired)`.
    pub fn to_client_config(&self) -> anyhow::Result<(EtcdClientConfig, CredentialState<BasicAuthMaterial>)> {
        // Auth: wire YAML username/password into CredentialState::Resolved so
        // `build_connect_options` picks it up.
        let (auth, auth_state): (Option<EtcdAuth>, CredentialState<BasicAuthMaterial>) = match &self.auth {
            Some(cfg) => (
                Some(EtcdAuth {
                    // Placeholder slot — `secret_ref` not used by the provider. The
                    // provider only checks `crd.auth.is_some()` to decide
                    // whether to authenticate; the material comes from
                    // `auth_state`, not from `secret_ref`.
                    slot: SecretSlot {
                        secret_ref: SecretObjectReference {
                            group: None,
                            kind: None,
                            name: "conf-center-yaml".to_string(),
                            namespace: None,
                        },
                        secret: None,
                    },
                }),
                CredentialState::Resolved(BasicAuthMaterial {
                    username: Some(cfg.username.clone()),
                    password: cfg.password.clone(),
                }),
            ),
            None => (None, CredentialState::NotRequired),
        };

        // TLS — read cert files from disk and wrap them in a synthetic Secret
        // so that the ConfCenter flow uses the same TLS material path as the
        // Gateway's CRD-driven LinkSys flow (Secret → TlsMaterial).
        let tls = if let Some(tls_cfg) = &self.tls {
            let ca = read_optional_file(&tls_cfg.ca_cert_file)?;
            let cert = read_optional_file(&tls_cfg.client_cert_file)?;
            let key = read_optional_file(&tls_cfg.client_key_file)?;

            let mut data = BTreeMap::new();
            if let Some(pem) = ca {
                data.insert(TLS_CA_KEY.to_string(), ByteString(pem.into_bytes()));
            }
            if let Some(pem) = cert {
                data.insert(TLS_CERT_KEY.to_string(), ByteString(pem.into_bytes()));
            }
            if let Some(pem) = key {
                data.insert(TLS_KEY_KEY.to_string(), ByteString(pem.into_bytes()));
            }

            let has_ca = data.contains_key(TLS_CA_KEY);
            let has_client = data.contains_key(TLS_CERT_KEY) && data.contains_key(TLS_KEY_KEY);
            let secret = if data.is_empty() {
                None
            } else {
                Some(Secret {
                    data: Some(data),
                    type_: Some("kubernetes.io/tls".to_string()),
                    ..Default::default()
                })
            };

            Some(TlsConfig {
                enabled: true,
                outbound: OutboundTlsLocal {
                    // When a CA file is present, declare a synthetic CA ref so that
                    // resolve_outbound_tls knows to use resolved_ca_certificates
                    // (Custom CA) rather than system roots.
                    validation: if has_ca {
                        Some(OutboundTlsValidation {
                            ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
                                group: String::new(),
                                kind: "Secret".to_string(),
                                name: "conf-center-ca".to_string(),
                                namespace: None,
                            }]),
                            ..Default::default()
                        })
                    } else {
                        None
                    },
                    ..Default::default()
                },
                resolved_ca_certificates: if has_ca { secret.clone().map(|s| vec![s]) } else { None },
                resolved_client_certificate: if has_client { secret } else { None },
            })
        } else {
            None
        };

        // Timeout / keep-alive: conf_center-scoped defaults are injected HERE,
        // not in the shared `build_connect_options` — that builder also serves
        // the gateway's operator-configured LinkSys etcd clients, whose behavior
        // must not change (spec §10.5).
        //
        // - dial: always bounded (default 5s) so a dead endpoint cannot hang init.
        // - h2 keep-alive: ALWAYS ON (default 5s ping / 3s ack). Spec §4.2 fact 2:
        //   the h2 PING is the only mechanism that kills a blackholed established
        //   stream — a per-request timeout cannot detect a hung lease keepalive,
        //   and leader-election safety depends on the blackhole dying. Operator
        //   values win; a half-configured pair is completed with the default for
        //   the missing half rather than being silently dropped by the builder's
        //   both-or-nothing rule.
        const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 5_000;
        const DEFAULT_KEEP_ALIVE_TIME_SECS: u64 = 5;
        const DEFAULT_KEEP_ALIVE_TIMEOUT_SECS: u64 = 3;
        let timeout = Some(EtcdTimeout {
            dial: Some(self.connect_timeout_ms.unwrap_or(DEFAULT_CONNECT_TIMEOUT_MS)),
            request: self.request_timeout_ms,
            keep_alive: None,
        });
        let keep_alive = Some(EtcdKeepAlive {
            time: Some(self.keep_alive_time_secs.unwrap_or(DEFAULT_KEEP_ALIVE_TIME_SECS)),
            timeout: Some(self.keep_alive_timeout_secs.unwrap_or(DEFAULT_KEEP_ALIVE_TIMEOUT_SECS)),
            permit_without_stream: Some(true),
        });

        Ok((
            EtcdClientConfig {
                endpoints: self.endpoints.clone(),
                auth,
                tls,
                timeout,
                keep_alive,
                namespace: None,
                auto_sync_interval: None,
                max_call_send_size: None,
                max_call_recv_size: None,
                user_agent: None,
                reject_old_cluster: None,
                observability: None,
            },
            auth_state,
        ))
    }
}

/// Read a file to string if the path is provided.
fn read_optional_file(path: &Option<String>) -> anyhow::Result<Option<String>> {
    match path {
        Some(p) => {
            let content =
                std::fs::read_to_string(p).map_err(|e| anyhow::anyhow!("Failed to read TLS file '{}': {}", p, e))?;
            Ok(Some(content))
        }
        None => Ok(None),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leader_election_defaults_are_valid() {
        let c = EtcdLeaderElectionConfig {
            identity: None,
            grpc_addr: "10.0.0.11:9090".into(),
            admin_addr: "10.0.0.11:8080".into(),
            ..Default::default()
        };
        assert_eq!(c.lease_duration_secs, 15);
        assert_eq!(c.renew_deadline_secs, 10);
        assert_eq!(c.retry_period_secs, 2);
        c.validate().expect("defaults must validate");
    }

    #[test]
    fn leader_election_rejects_bad_timing() {
        // renew_deadline >= lease_duration
        let c = EtcdLeaderElectionConfig {
            grpc_addr: "10.0.0.11:9090".into(),
            admin_addr: "10.0.0.11:8080".into(),
            lease_duration_secs: 10,
            renew_deadline_secs: 10,
            retry_period_secs: 2,
            identity: None,
        };
        assert!(c.validate().is_err());
        // fewer than 3 keepalive attempts: floor(4/2)=2
        let c = EtcdLeaderElectionConfig {
            grpc_addr: "10.0.0.11:9090".into(),
            admin_addr: "10.0.0.11:8080".into(),
            lease_duration_secs: 6,
            renew_deadline_secs: 4,
            retry_period_secs: 2,
            identity: None,
        };
        assert!(c.validate().is_err());
    }

    #[test]
    fn leader_election_rejects_loopback_and_missing_addr() {
        let c = EtcdLeaderElectionConfig {
            grpc_addr: "127.0.0.1:9090".into(),
            admin_addr: "10.0.0.11:8080".into(),
            identity: None,
            ..Default::default()
        };
        assert!(c.validate().is_err(), "loopback grpc_addr must be rejected");
        let c = EtcdLeaderElectionConfig {
            grpc_addr: "".into(),
            admin_addr: "10.0.0.11:8080".into(),
            identity: None,
            ..Default::default()
        };
        assert!(c.validate().is_err(), "empty grpc_addr must be rejected");
    }

    #[test]
    fn test_deserialize_minimal() {
        let yaml = r#"
endpoints: ["http://127.0.0.1:2379"]
"#;
        let config: EtcdConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.endpoints, vec!["http://127.0.0.1:2379"]);
        assert_eq!(config.key_prefix, "/edgion");
        assert!(config.auth.is_none());
        assert!(config.tls.is_none());
    }

    #[test]
    fn test_deserialize_full() {
        let yaml = r#"
endpoints: ["http://etcd-1:2379", "http://etcd-2:2379"]
key_prefix: /my-app
connect_timeout_ms: 3000
request_timeout_ms: 8000
keep_alive_time_secs: 30
keep_alive_timeout_secs: 10
auth:
  username: root
  password: changeme
"#;
        let config: EtcdConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.endpoints.len(), 2);
        assert_eq!(config.key_prefix, "/my-app");
        assert_eq!(config.connect_timeout_ms, Some(3000));

        let auth = config.auth.as_ref().unwrap();
        assert_eq!(auth.username, "root");
        assert_eq!(auth.password, "changeme");
    }

    #[test]
    fn test_default_key_prefix() {
        assert_eq!(default_key_prefix(), "/edgion");
    }

    #[test]
    fn test_to_client_config_minimal() {
        let config = EtcdConfig {
            endpoints: vec!["http://127.0.0.1:2379".to_string()],
            key_prefix: "/edgion".to_string(),
            auth: None,
            tls: None,
            connect_timeout_ms: None,
            request_timeout_ms: None,
            keep_alive_time_secs: None,
            keep_alive_timeout_secs: None,
            leader_election: EtcdLeaderElectionConfig::default(),
        };

        let (client_config, auth_state) = config.to_client_config().unwrap();
        assert_eq!(client_config.endpoints, vec!["http://127.0.0.1:2379"]);
        assert!(client_config.auth.is_none());
        assert!(client_config.tls.is_none());
        // Conf_center-scoped defaults are injected at THIS layer (never in the
        // shared builder — spec §10.5): a minimal config still gets a bounded
        // dial timeout and always-on h2 keepalive (spec §4.2 fact 2), with no
        // channel-wide request timeout (it would kill the blocking campaign()).
        let timeout = client_config.timeout.as_ref().expect("dial default expected");
        assert_eq!(timeout.dial, Some(5_000));
        assert!(timeout.request.is_none());
        let ka = client_config.keep_alive.as_ref().expect("keepalive default expected");
        assert_eq!((ka.time, ka.timeout), (Some(5), Some(3)));
        assert_eq!(ka.permit_without_stream, Some(true));
        assert!(matches!(auth_state, CredentialState::NotRequired));
    }

    #[test]
    fn test_to_client_config_with_auth_wires_yaml_credentials_into_state() {
        // YAML `conf_center.auth` must flow through to `CredentialState::Resolved`.
        let config = EtcdConfig {
            endpoints: vec!["http://127.0.0.1:2379".to_string()],
            key_prefix: "/edgion".to_string(),
            auth: Some(EtcdAuthConfig {
                username: "root".to_string(),
                password: "changeme".to_string(),
            }),
            tls: None,
            connect_timeout_ms: Some(5000),
            request_timeout_ms: Some(10000),
            keep_alive_time_secs: None,
            keep_alive_timeout_secs: None,
            leader_election: EtcdLeaderElectionConfig::default(),
        };

        let (client_config, auth_state) = config.to_client_config().unwrap();

        assert!(
            client_config.auth.is_some(),
            "placeholder EtcdAuth must be present so the provider's `crd.auth.is_some()` gate opens"
        );
        match auth_state {
            CredentialState::Resolved(mat) => {
                assert_eq!(mat.username.as_deref(), Some("root"));
                assert_eq!(mat.password, "changeme");
            }
            other => panic!("expected Resolved, got {:?}", other),
        }

        let timeout = client_config.timeout.unwrap();
        assert_eq!(timeout.dial, Some(5000));
        assert_eq!(timeout.request, Some(10000));
    }

    #[test]
    fn test_to_client_config_without_auth_is_not_required() {
        let config = EtcdConfig {
            endpoints: vec!["http://127.0.0.1:2379".to_string()],
            key_prefix: "/edgion".to_string(),
            auth: None,
            tls: None,
            connect_timeout_ms: None,
            request_timeout_ms: None,
            keep_alive_time_secs: None,
            keep_alive_timeout_secs: None,
            leader_election: EtcdLeaderElectionConfig::default(),
        };

        let (client_config, auth_state) = config.to_client_config().unwrap();
        assert!(client_config.auth.is_none());
        assert!(matches!(auth_state, CredentialState::NotRequired));
    }

    /// A CA-only TLS config (only ca_cert_file set, no client cert/key) must:
    /// - produce a TlsConfig with validation.caCertificateRefs set so the resolver
    ///   reads the CA from resolved_ca_certificates (not system roots).
    /// - have resolved_ca_certificates set (the CA material).
    /// - have resolved_client_certificate = None (no mTLS identity).
    ///
    /// Regression guard for the bug where the synthetic Secret was assigned to
    /// BOTH resolved_ca_certificates and resolved_client_certificate, causing the
    /// resolver to fail with OutboundTlsClientCertInvalid when no tls.crt/tls.key
    /// were present.
    #[test]
    fn test_ca_only_tls_config_does_not_set_client_secret() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Write a dummy CA PEM to a temp file.
        let mut ca_file = NamedTempFile::new().expect("failed to create temp file");
        write!(
            ca_file,
            "-----BEGIN CERTIFICATE-----\nMIIBxxx\n-----END CERTIFICATE-----\n"
        )
        .expect("failed to write temp CA");
        let ca_path = ca_file.path().to_str().unwrap().to_string();

        let config = EtcdConfig {
            endpoints: vec!["https://etcd:2379".to_string()],
            key_prefix: "/edgion".to_string(),
            auth: None,
            tls: Some(EtcdTlsConfig {
                ca_cert_file: Some(ca_path),
                client_cert_file: None,
                client_key_file: None,
            }),
            connect_timeout_ms: None,
            request_timeout_ms: None,
            keep_alive_time_secs: None,
            keep_alive_timeout_secs: None,
            leader_election: EtcdLeaderElectionConfig::default(),
        };

        let (client_config, _) = config.to_client_config().unwrap();
        let tls = client_config.tls.expect("TLS config must be present");

        // When a CA file is configured, the outbound validation block must contain a
        // non-empty ca_certificate_refs so that resolve_outbound_tls picks Custom CA.
        assert!(
            tls.outbound
                .validation
                .as_ref()
                .and_then(|v| v.ca_certificate_refs.as_ref())
                .is_some_and(|r| !r.is_empty()),
            "outbound.validation.ca_certificate_refs must be set when ca_cert_file is provided"
        );
        assert!(
            tls.resolved_ca_certificates.is_some(),
            "resolved_ca_certificates must carry the CA material"
        );
        assert!(
            tls.resolved_client_certificate.is_none(),
            "resolved_client_certificate must be None when no client cert/key is configured (CA-only)"
        );
    }
}
