//! Etcd ConfCenter configuration schema types.
//!
//! YAML-friendly (snake_case) configuration for the etcd backend. This module
//! holds only the pure serde schema; the conversion to the gateway's
//! `EtcdClientConfig` (`EtcdConfig::to_client_config`) stays in the controller
//! because it depends on gateway LinkSys types.

use serde::{Deserialize, Serialize};

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
}

pub(crate) fn default_key_prefix() -> String {
    "/edgion".to_string()
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
// Tests (schema only — conversion tests live with the impl in the controller)
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
    fn test_default_key_prefix() {
        assert_eq!(default_key_prefix(), "/edgion");
    }
}
