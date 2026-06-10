use crate::core::common::auth::AdminAuthConfig;
use crate::core::common::config::{AdminTlsConfig, ConfSyncSecurityConfig};
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
    /// HTTP listen address for the dedicated liveness/readiness probe listener
    pub probe_addr: String,
    /// HTTP listen address for the dedicated Prometheus metrics listener
    pub metrics_addr: String,
    /// Optional server-side TLS for the admin HTTP API.
    /// When omitted, the admin listener serves plain HTTP.
    /// When set, the admin listener serves HTTPS; probe and metrics listeners remain HTTP.
    pub admin_tls: Option<AdminTlsConfig>,
    /// CIDR allowlist for the admin HTTP API. Empty/unset = allow all.
    /// Matched against the TCP peer address only; X-Forwarded-For is never trusted.
    #[serde(default)]
    pub allow_admin_ips: Vec<String>,
    /// L4 TCP-layer allowlist: peers not matching are dropped before the TLS
    /// handshake on the HTTPS admin listener. Independent from `allow_admin_ips`
    /// (L7). Empty/unset = allow all. Matched against the TCP peer address only.
    #[serde(default)]
    pub allow_tcp_ips: Vec<String>,
}

impl Default for CenterServerConfig {
    fn default() -> Self {
        Self {
            grpc_addr: "0.0.0.0:12251".to_string(),
            http_addr: "0.0.0.0:12201".to_string(),
            probe_addr: "0.0.0.0:12200".to_string(),
            metrics_addr: "0.0.0.0:12290".to_string(),
            admin_tls: None,
            allow_admin_ips: Vec::new(),
            allow_tcp_ips: Vec::new(),
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
        assert_eq!(config.server.grpc_addr, "0.0.0.0:12251");
        assert_eq!(config.server.http_addr, "0.0.0.0:12201");
        assert_eq!(config.server.probe_addr, "0.0.0.0:12200");
        assert_eq!(config.server.metrics_addr, "0.0.0.0:12290");
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

    #[test]
    fn test_admin_tls_deserializes_from_yaml() {
        let yaml = r#"
server:
  http_addr: "0.0.0.0:5900"
  admin_tls:
    cert: "certs/admin/server.crt"
    key: "certs/admin/server.key"
"#;
        let cfg: CenterConfig = serde_yaml::from_str(yaml).unwrap();
        let tls = cfg.server.admin_tls.expect("admin_tls should parse");
        assert_eq!(tls.cert, "certs/admin/server.crt");
        assert_eq!(tls.key, "certs/admin/server.key");
    }

    #[test]
    fn test_admin_tls_absent_is_none() {
        let yaml = r#"
server:
  http_addr: "0.0.0.0:5900"
"#;
        let cfg: CenterConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.server.admin_tls.is_none(), "omitted admin_tls must be None");
    }

    /// Parity guard: the production config load (`EdgionCenterCli::run`) does NOT use
    /// `serde_yaml::from_str` — it uses `singleton_map_recursive::deserialize`, which
    /// changes how enums/maps are treated. The other `admin_tls` tests use `from_str`,
    /// so this test exercises the REAL production deserializer to ensure
    /// `Option<AdminTlsConfig>` round-trips identically (Some with correct fields when
    /// present, None when absent).
    #[test]
    fn test_admin_tls_parses_via_production_singleton_deserializer() {
        let yaml = r#"
server:
  http_addr: "0.0.0.0:5900"
  admin_tls:
    cert: "certs/admin/server.crt"
    key: "certs/admin/server.key"
"#;
        let de = serde_yaml::Deserializer::from_str(yaml);
        let cfg: CenterConfig = serde_yaml::with::singleton_map_recursive::deserialize(de)
            .expect("admin_tls must parse through the production singleton_map_recursive path");
        let tls = cfg
            .server
            .admin_tls
            .expect("admin_tls Some via singleton_map_recursive");
        assert_eq!(tls.cert, "certs/admin/server.crt");
        assert_eq!(tls.key, "certs/admin/server.key");

        let de_absent = serde_yaml::Deserializer::from_str("server:\n  http_addr: \"0.0.0.0:5900\"\n");
        let cfg_absent: CenterConfig = serde_yaml::with::singleton_map_recursive::deserialize(de_absent).unwrap();
        assert!(
            cfg_absent.server.admin_tls.is_none(),
            "omitted admin_tls must be None via singleton_map_recursive"
        );
    }
}

#[cfg(test)]
mod allow_tcp_ips_tests {
    use super::*;

    #[test]
    fn default_allow_tcp_ips_is_empty() {
        let c = CenterServerConfig::default();
        assert!(c.allow_tcp_ips.is_empty());
    }

    #[test]
    fn yaml_without_field_defaults_to_empty() {
        // Omitting allow_tcp_ips must deserialize to an empty Vec (backward compat).
        let yaml = "grpc_addr: \"0.0.0.0:1\"\nhttp_addr: \"0.0.0.0:2\"\nprobe_addr: \"0.0.0.0:3\"\nmetrics_addr: \"0.0.0.0:4\"\n";
        let c: CenterServerConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(c.allow_tcp_ips.is_empty());
    }

    #[test]
    fn yaml_with_field_parses() {
        let yaml = "allow_tcp_ips:\n  - \"10.0.0.0/8\"\n";
        let c: CenterServerConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(c.allow_tcp_ips, vec!["10.0.0.0/8".to_string()]);
    }
}
