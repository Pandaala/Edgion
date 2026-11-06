//! Etcd ConfCenter configuration schema + client-config conversion.
//!
//! YAML-friendly (snake_case) configuration for the etcd backend, plus the
//! conversion to the explicit trusted `EtcdBootstrapConfig` input.
//! This schema lives in the controller (`crate::config::conf_center::etcd`); the
//! etcd link client and bootstrap input types it depends on live in `edgion-common`.

use edgion_common::link_sys_core::providers::etcd::EtcdBootstrapConfig;
use edgion_resources::credentials::materials::{BasicAuthMaterial, TlsMaterial};
use edgion_resources::resources::link_sys::etcd::{EtcdKeepAlive, EtcdTimeout};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

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
///   controller_name: edgion.io/gateway-controller
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
    /// Explicit Gateway API identity of this deployment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller_name: Option<String>,
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

    /// Deadline (seconds) for the initial load of every resource kind to reach
    /// readiness. This is the only startup bound; phase waits have no inner
    /// deadline. Must be > 30.
    #[serde(default = "super::readiness::default_cache_ready_deadline_secs")]
    pub cache_ready_deadline_secs: u64,
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
        use edgion_common::egress::{classify_ip, IpClass};

        if addr.is_empty() {
            return false;
        }
        // Reject loopback / unspecified host portions. A leader advertising
        // 127.0.0.1 or 0.0.0.0 yields a 409 body / rotation target that can
        // never work (spec §8).
        let host = addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(addr);
        let host = host.trim_start_matches('[').trim_end_matches(']');
        if host == "localhost" {
            return false;
        }
        match host.parse() {
            Ok(ip) => !matches!(classify_ip(ip), IpClass::Loopback | IpClass::Unspecified),
            Err(_) => true, // DNS names remain supported without resolution here.
        }
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
// Conversion to the trusted Controller bootstrap input
// ============================================================================

impl EtcdConfig {
    /// Fail-fast validation of startup timing knobs.
    pub fn validate(&self) -> anyhow::Result<()> {
        super::readiness::validate_cache_ready_deadline_secs("etcd", self.cache_ready_deadline_secs)
    }

    /// Read local PEM files and retain direct bootstrap credentials without
    /// creating resource references or synthetic Secrets.
    pub fn to_bootstrap_config(&self) -> anyhow::Result<EtcdBootstrapConfig> {
        let auth = self.auth.as_ref().map(|cfg| BasicAuthMaterial {
            username: Some(cfg.username.clone()),
            password: cfg.password.clone(),
        });
        let tls = if let Some(cfg) = &self.tls {
            let ca = read_optional_file(&cfg.ca_cert_file)?.map(String::into_bytes);
            let cert = read_optional_file(&cfg.client_cert_file)?.map(String::into_bytes);
            let key = read_optional_file(&cfg.client_key_file)?.map(|key| Zeroizing::new(key.into_bytes()));
            // Preserve the established partial-pair behavior: all specified
            // files are read, but mTLS is selected only for a complete pair.
            let (client_cert_pem, client_key_pem) = match cert.zip(key) {
                Some((cert, key)) => (Some(cert), Some(key)),
                None => (None, None),
            };
            Some(TlsMaterial {
                ca_pem: ca,
                client_cert_pem,
                client_key_pem,
            })
        } else {
            None
        };

        // Timeout / keep-alive: conf_center-scoped defaults are injected HERE,
        // not in the shared options mapper — that builder also serves
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
        });
        let keep_alive = Some(EtcdKeepAlive {
            time: Some(self.keep_alive_time_secs.unwrap_or(DEFAULT_KEEP_ALIVE_TIME_SECS)),
            timeout: Some(self.keep_alive_timeout_secs.unwrap_or(DEFAULT_KEEP_ALIVE_TIMEOUT_SECS)),
            permit_without_stream: Some(true),
        });

        Ok(EtcdBootstrapConfig {
            endpoints: self.endpoints.clone(),
            auth,
            tls,
            timeout,
            keep_alive,
            namespace: None,
        })
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
#[path = "../../../tests/unit/config/conf_center/etcd_test.rs"]
mod tests;
