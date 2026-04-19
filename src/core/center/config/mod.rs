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
    /// How often (seconds) to sync RegionRoute snapshots from online controllers.
    pub sync_interval_secs: u64,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            sqlite_path: "data/center.db".to_string(),
            enabled: true,
            sync_interval_secs: 60,
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
    /// Omit to use default local auth (admin/edgion1234).
    /// Both `auth` and `local_auth` can be enabled simultaneously.
    #[serde(default)]
    pub local_auth: Option<LocalAuthConfig>,
    /// TLS config for the federation gRPC server.
    /// Omit this section to keep plaintext gRPC (backward compatible).
    #[serde(default)]
    pub grpc_security: ConfSyncSecurityConfig,
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
    /// How often to send ListRequest to each controller (seconds, default: 300 = 5min)
    pub list_interval_secs: u64,
    /// Timeout waiting for ListResponse (seconds, default: 30)
    pub list_timeout_secs: u64,
    /// Timeout waiting for CommandResponse (seconds, default: 30)
    pub command_timeout_secs: u64,
    /// Heartbeat ping interval sent to controllers (seconds, default: 30)
    pub ping_interval_secs: u64,
}

impl Default for CenterSyncConfig {
    fn default() -> Self {
        Self {
            list_interval_secs: 300,
            list_timeout_secs: 30,
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
        assert_eq!(config.sync.list_interval_secs, 300);
        assert_eq!(config.sync.ping_interval_secs, 30);
        assert_eq!(config.sync.list_timeout_secs, 30);
        assert_eq!(config.sync.command_timeout_secs, 30);
    }

    #[test]
    fn test_center_config_parses_from_yaml() {
        let yaml = r#"
server:
  grpc_addr: "0.0.0.0:50100"
  http_addr: "0.0.0.0:5900"
sync:
  list_interval_secs: 60
"#;
        let config: CenterConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.server.grpc_addr, "0.0.0.0:50100");
        assert_eq!(config.sync.list_interval_secs, 60);
        assert_eq!(config.sync.ping_interval_secs, 30); // default
    }
}
