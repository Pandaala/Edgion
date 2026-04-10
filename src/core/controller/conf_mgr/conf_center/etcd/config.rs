//! Etcd ConfCenter configuration types
//!
//! Provides TOML-friendly (snake_case) configuration for the etcd backend.
//! Converts to `EtcdClientConfig` internally for `EtcdLinkClient::from_config()`.

use serde::{Deserialize, Serialize};

use crate::core::controller::conf_mgr::conf_center::common::EndpointMode;
use crate::types::resources::link_sys::etcd::{
    EtcdAuth, EtcdClientConfig, EtcdKeepAlive, EtcdTimeout, EtcdTls, EtcdTlsCerts,
};

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
/// endpoint_mode = "endpoint_slice"
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

    /// Endpoint discovery mode (default: Auto → EndpointSlice)
    #[serde(default)]
    pub endpoint_mode: EndpointMode,

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
    /// Get endpoint mode
    pub fn endpoint_mode(&self) -> EndpointMode {
        self.endpoint_mode
    }

    /// Convert to `EtcdClientConfig` that `EtcdLinkClient::from_config()` accepts.
    ///
    /// TLS certificate file paths are read from disk and embedded as PEM strings.
    pub fn to_client_config(&self) -> anyhow::Result<EtcdClientConfig> {
        // Auth
        let auth = self.auth.as_ref().map(|a| EtcdAuth {
            username: Some(a.username.clone()),
            password: Some(a.password.clone()),
            secret_ref: None,
        });

        // TLS — read cert files from disk
        let tls = if let Some(tls_cfg) = &self.tls {
            let ca_cert = read_optional_file(&tls_cfg.ca_cert_file)?;
            let client_cert = read_optional_file(&tls_cfg.client_cert_file)?;
            let client_key = read_optional_file(&tls_cfg.client_key_file)?;

            Some(EtcdTls {
                enabled: true,
                certs: Some(EtcdTlsCerts {
                    ca_cert,
                    client_cert,
                    client_key,
                    secret_ref: None,
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
        let keep_alive =
            if self.keep_alive_time_secs.is_some() || self.keep_alive_timeout_secs.is_some() {
                Some(EtcdKeepAlive {
                    time: self.keep_alive_time_secs,
                    timeout: self.keep_alive_timeout_secs,
                    permit_without_stream: Some(true),
                })
            } else {
                None
            };

        Ok(EtcdClientConfig {
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
        })
    }
}

/// Read a file to string if the path is provided.
fn read_optional_file(path: &Option<String>) -> anyhow::Result<Option<String>> {
    match path {
        Some(p) => {
            let content = std::fs::read_to_string(p)
                .map_err(|e| anyhow::anyhow!("Failed to read TLS file '{}': {}", p, e))?;
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
        let toml = r#"
endpoints = ["http://127.0.0.1:2379"]
"#;
        let config: EtcdConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.endpoints, vec!["http://127.0.0.1:2379"]);
        assert_eq!(config.key_prefix, "/edgion");
        assert!(config.endpoint_mode.is_auto());
        assert!(config.auth.is_none());
        assert!(config.tls.is_none());
    }

    #[test]
    fn test_deserialize_full() {
        let toml = r#"
endpoints = ["http://etcd-1:2379", "http://etcd-2:2379"]
key_prefix = "/my-app"
endpoint_mode = "endpoint_slice"
connect_timeout_ms = 3000
request_timeout_ms = 8000
keep_alive_time_secs = 30
keep_alive_timeout_secs = 10

[auth]
username = "root"
password = "changeme"
"#;
        let config: EtcdConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.endpoints.len(), 2);
        assert_eq!(config.key_prefix, "/my-app");
        assert!(config.endpoint_mode.is_endpoint_slice());
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
            endpoint_mode: EndpointMode::default(),
            auth: None,
            tls: None,
            connect_timeout_ms: None,
            request_timeout_ms: None,
            keep_alive_time_secs: None,
            keep_alive_timeout_secs: None,
        };

        let client_config = config.to_client_config().unwrap();
        assert_eq!(client_config.endpoints, vec!["http://127.0.0.1:2379"]);
        assert!(client_config.auth.is_none());
        assert!(client_config.tls.is_none());
        assert!(client_config.timeout.is_none());
    }

    #[test]
    fn test_to_client_config_with_auth() {
        let config = EtcdConfig {
            endpoints: vec!["http://127.0.0.1:2379".to_string()],
            key_prefix: "/edgion".to_string(),
            endpoint_mode: EndpointMode::default(),
            auth: Some(EtcdAuthConfig {
                username: "user".to_string(),
                password: "pass".to_string(),
            }),
            tls: None,
            connect_timeout_ms: Some(5000),
            request_timeout_ms: Some(10000),
            keep_alive_time_secs: None,
            keep_alive_timeout_secs: None,
        };

        let client_config = config.to_client_config().unwrap();
        let auth = client_config.auth.unwrap();
        assert_eq!(auth.username, Some("user".to_string()));
        assert_eq!(auth.password, Some("pass".to_string()));
        let timeout = client_config.timeout.unwrap();
        assert_eq!(timeout.dial, Some(5000));
        assert_eq!(timeout.request, Some(10000));
    }

    #[test]
    fn test_default_key_prefix() {
        assert_eq!(default_key_prefix(), "/edgion");
    }
}
