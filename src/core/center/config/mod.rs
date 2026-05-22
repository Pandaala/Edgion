use crate::core::common::auth::AdminAuthConfig;
use crate::core::common::config::ConfSyncSecurityConfig;
use crate::core::common::local_auth::LocalAuthConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    /// Path to the SQLite database file. Relative paths are resolved from the current working directory.
    pub sqlite_path: String,
    /// Whether the SQLite database is enabled.
    pub enabled: bool,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            sqlite_path: "data/center.db".to_string(),
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CenterConfig {
    pub server: CenterServerConfig,
    pub sync: CenterSyncConfig,
    /// SQLite database configuration.
    #[serde(default)]
    pub database: DatabaseConfig,
    /// Optional OIDC Admin API authentication.
    /// Omit to disable OIDC auth. When present, Admin API endpoints (except
    /// health/ready) require a valid JWT Bearer token from the OIDC provider.
    #[serde(default)]
    pub auth: Option<AdminAuthConfig>,
    /// Optional local Admin API authentication.
    /// Center does not inject any default credentials; omit this section only
    /// when OIDC `auth` is configured, otherwise the Admin API will reject all
    /// requests. Both `auth` and `local_auth` can be enabled simultaneously.
    #[serde(default)]
    pub local_auth: Option<LocalAuthConfig>,
    /// TLS config for the federation gRPC server.
    /// Federation requires mTLS; the server refuses to start without a valid TLS block.
    #[serde(default)]
    pub grpc_security: ConfSyncSecurityConfig,
    /// Server-side peer-identity verification. Required (with a non-empty
    /// trust_domain) under mTLS.
    #[serde(default)]
    pub peer_identity: Option<PeerIdentityConfig>,
}

impl Default for CenterConfig {
    fn default() -> Self {
        Self {
            server: CenterServerConfig::default(),
            sync: CenterSyncConfig::default(),
            database: DatabaseConfig::default(),
            auth: None,
            local_auth: None,
            grpc_security: ConfSyncSecurityConfig::default(),
            peer_identity: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PeerIdentityConfig {
    /// SPIFFE trust domain expected in the peer cert's URI SAN
    /// (`spiffe://{trust_domain}/controllers/{cluster}/{name}`).
    pub trust_domain: String,
}

impl Default for PeerIdentityConfig {
    fn default() -> Self {
        Self {
            trust_domain: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CenterServerConfig {
    /// gRPC listen address for federation sync
    pub grpc_addr: String,
    /// HTTP listen address for admin API
    pub http_addr: String,
}

impl Default for CenterServerConfig {
    fn default() -> Self {
        Self {
            grpc_addr: "0.0.0.0:50052".to_string(),
            http_addr: "0.0.0.0:5810".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CenterSyncConfig {
    /// Timeout waiting for CommandResponse (seconds, default: 30)
    pub command_timeout_secs: u64,
    /// Heartbeat ping interval sent to controllers (seconds, default: 30)
    pub ping_interval_secs: u64,
}

impl Default for CenterSyncConfig {
    fn default() -> Self {
        Self {
            command_timeout_secs: 30,
            ping_interval_secs: 30,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_center_config_defaults() {
        let config = CenterConfig::default();
        assert_eq!(config.server.grpc_addr, "0.0.0.0:50052");
        assert_eq!(config.server.http_addr, "0.0.0.0:5810");
        assert_eq!(config.sync.ping_interval_secs, 30);
        assert_eq!(config.sync.command_timeout_secs, 30);
    }

    #[test]
    fn test_center_config_parses_from_yaml() {
        let yaml = r#"
server:
  grpc_addr: "0.0.0.0:50100"
  http_addr: "0.0.0.0:5900"
sync:
  ping_interval_secs: 60
"#;
        let config: CenterConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.server.grpc_addr, "0.0.0.0:50100");
        assert_eq!(config.sync.ping_interval_secs, 60);
        assert_eq!(config.sync.command_timeout_secs, 30); // default
    }

    #[test]
    fn default_peer_identity_is_none() {
        let c = CenterConfig::default();
        assert!(c.peer_identity.is_none());
    }

    #[test]
    fn parses_peer_identity() {
        let yaml = r#"
peer_identity:
  trust_domain: "edgion.io"
"#;
        let c: CenterConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(c.peer_identity.unwrap().trust_domain, "edgion.io");
    }
}
