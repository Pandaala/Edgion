//! Etcd ConfCenter configuration types
//!
//! Provides TOML-friendly (snake_case) configuration for the etcd backend.
//! Converts to `EtcdClientConfig` internally for `EtcdLinkClient::from_config()`.

use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::ByteString;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::core::gateway::link_sys::credential::CredentialState;
use crate::types::credentials::materials::BasicAuthMaterial;
use crate::types::credentials::resolvers::tls::{TLS_CA_KEY, TLS_CERT_KEY, TLS_KEY_KEY};
use crate::types::resources::common::{SecretObjectReference, SecretSlot, TlsCerts, TlsConfig};
use crate::types::resources::link_sys::etcd::{EtcdAuth, EtcdClientConfig, EtcdKeepAlive, EtcdTimeout};

// ============================================================================
// EtcdConfig — TOML top-level config
// ============================================================================

/// Etcd configuration center settings
///
/// ## Example (TOML)
///
/// ```toml
/// [conf_center]
/// type = "etcd"
/// endpoints = ["http://127.0.0.1:2379"]
/// key_prefix = "/edgion"
///
/// [conf_center.auth]
/// username = "root"
/// password = "changeme"
///
/// [conf_center.tls]
/// ca_cert_file = "/etc/edgion/certs/etcd-ca.pem"
/// client_cert_file = "/etc/edgion/certs/etcd-client.pem"
/// client_key_file = "/etc/edgion/certs/etcd-client-key.pem"
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
}

fn default_key_prefix() -> String {
    "/edgion".to_string()
}

/// Authentication configuration for etcd (TOML snake_case)
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

/// TLS configuration for etcd (TOML snake_case, file paths)
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
    /// Auth note: Controller's ConfCenter uses file-based credentials (TOML
    /// `[conf_center.auth]`), not `SecretObjectReference`. To reuse the same
    /// `build_connect_options` code path as the LinkSys CRD flow, we:
    /// - fill `EtcdClientConfig.auth` with a synthetic `EtcdAuth` placeholder
    ///   (the `secret_ref` is never read by the Gateway provider; only
    ///   `crd.auth.is_some()` is inspected as the "auth configured" gate);
    /// - return a [`CredentialState::Resolved`] carrying the TOML material so
    ///   the provider's auth arm gets the plaintext `username` / `password`.
    ///
    /// If no `[conf_center.auth]` section is configured, returns
    /// `(EtcdClientConfig { auth: None, .. }, CredentialState::NotRequired)`.
    pub fn to_client_config(&self) -> anyhow::Result<(EtcdClientConfig, CredentialState<BasicAuthMaterial>)> {
        // Auth: wire TOML username/password into CredentialState::Resolved so
        // `build_connect_options` picks it up — previously this path hard-coded
        // `auth = None` which silently dropped the TOML credentials (a
        // silent-fallback regression of the credential-state contract).
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
                            name: "conf-center-toml".to_string(),
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
                certs: Some(TlsCerts {
                    ca_cert: None,
                    client_cert: None,
                    secret_ref: None,
                    secret,
                }),
                insecure_skip_verify: None,
            })
        } else {
            None
        };

        // Timeout
        let timeout = if self.connect_timeout_ms.is_some() || self.request_timeout_ms.is_some() {
            Some(EtcdTimeout {
                dial: self.connect_timeout_ms,
                request: self.request_timeout_ms,
                keep_alive: None,
            })
        } else {
            None
        };

        // Keep-alive
        let keep_alive = if self.keep_alive_time_secs.is_some() || self.keep_alive_timeout_secs.is_some() {
            Some(EtcdKeepAlive {
                time: self.keep_alive_time_secs,
                timeout: self.keep_alive_timeout_secs,
                permit_without_stream: Some(true),
            })
        } else {
            None
        };

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
        };

        let (client_config, auth_state) = config.to_client_config().unwrap();
        assert_eq!(client_config.endpoints, vec!["http://127.0.0.1:2379"]);
        assert!(client_config.auth.is_none());
        assert!(client_config.tls.is_none());
        assert!(client_config.timeout.is_none());
        assert!(matches!(auth_state, CredentialState::NotRequired));
    }

    #[test]
    fn test_to_client_config_with_auth_wires_toml_credentials_into_state() {
        // Regression guard (B1): TOML `[conf_center.auth]` must flow through
        // to `CredentialState::Resolved` — previously it was silently dropped.
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
        };

        let (client_config, auth_state) = config.to_client_config().unwrap();
        assert!(client_config.auth.is_none());
        assert!(matches!(auth_state, CredentialState::NotRequired));
    }

    #[test]
    fn test_default_key_prefix() {
        assert_eq!(default_key_prefix(), "/edgion");
    }
}
