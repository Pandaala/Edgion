//! Shared DNS resolver used by both the controller (background resolution)
//! and the gateway (data-plane cache). Backend-agnostic; the standard-DNS
//! backend uses hickory-resolver, HTTP DNS reuses link_sys_core's HTTP client,
//! and the no-config path falls back to the system resolver.

mod hickory;
pub mod http_fetcher;
pub mod httpdns;
pub mod resolver_select;
pub mod standard;
pub mod system;

use std::net::IpAddr;
use std::sync::Arc;

use edgion_resources::resources::link_sys::HttpDnsResolverConfig;

/// Configuration-owned HTTP DNS generation. Querying never builds clients or
/// reads inherited TLS policy; an unusable generation retains its own fallback.
pub struct PreparedHttpDns {
    config: HttpDnsResolverConfig,
    fetcher: Option<http_fetcher::ReqwestHttpDnsFetcher>,
    address_fingerprint: String,
    runtime_identity: [u8; 32],
}

impl std::fmt::Debug for PreparedHttpDns {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedHttpDns")
            .field("ready", &self.is_ready())
            .finish_non_exhaustive()
    }
}

impl PreparedHttpDns {
    /// Cold configuration preparation. Even invalid input publishes a new state.
    pub fn prepare(mut config: HttpDnsResolverConfig) -> Arc<Self> {
        let defaults = crate::link_sys_core::outbound_tls::outbound_global_default();
        let runtime_identity = Self::identity_for(&config, &defaults);
        let address_fingerprint = {
            let _redact = edgion_resources::redact::AdminRedactGuard::new();
            format!("httpdns:{}", serde_json::to_string(&config).unwrap_or_default())
        };
        let fetcher = if config.validate().is_empty() {
            http_fetcher::ReqwestHttpDnsFetcher::with_defaults(config.connection.as_ref(), defaults).ok()
        } else {
            None
        };
        // Connection policy and resolved Secret bodies are consumed only by the
        // native client. Do not retain another copy in the query generation.
        config.connection = None;
        Arc::new(Self {
            config,
            fetcher,
            address_fingerprint,
            runtime_identity,
        })
    }

    pub fn is_ready(&self) -> bool {
        self.fetcher.is_some()
    }

    /// Called only on configuration publication or Controller refresh ticks.
    pub fn matches_config(&self, config: &HttpDnsResolverConfig) -> bool {
        self.runtime_identity
            == Self::identity_for(config, &crate::link_sys_core::outbound_tls::outbound_global_default())
    }

    fn identity_for(
        config: &HttpDnsResolverConfig,
        defaults: &edgion_resources::resources::common::OutboundTlsConfig,
    ) -> [u8; 32] {
        use crate::link_sys_core::fingerprint::{hash_tls_material, hash_tls_policy, FingerprintHasher};
        let mut hash = FingerprintHasher::new();
        {
            let _redact = edgion_resources::redact::AdminRedactGuard::new();
            hash.bytes(&serde_json::to_vec(config).unwrap_or_default());
        }
        let local = config
            .connection
            .as_ref()
            .and_then(|connection| connection.tls.as_ref());
        hash_tls_material(&mut hash, local);
        if local.is_some_and(|tls| tls.enabled) {
            let global = defaults.clone();
            let inherited = edgion_resources::resources::common::TlsConfig {
                enabled: true,
                outbound: edgion_resources::resources::common::OutboundTlsLocal {
                    verify: Some(global.verify),
                    validation: global.validation,
                    client_certificate_ref: global.client_certificate_ref,
                },
                resolved_ca_certificates: global.resolved_ca_certificates,
                resolved_client_certificate: global.resolved_client_certificate,
            };
            hash_tls_policy(&mut hash, Some(&inherited));
            hash_tls_material(&mut hash, Some(&inherited));
        }
        hash.finish()
    }
}

/// Strip a single pair of surrounding `[` `]` brackets, if present (bracketed
/// IPv6 literal without a port, e.g. `"[::1]"`). Shared by the standard-DNS
/// server-string parser and the HTTP DNS response-body IP parser so a
/// bracketed IPv6 literal parses the same way in both places.
pub(crate) fn strip_brackets(s: &str) -> &str {
    s.strip_prefix('[').and_then(|s| s.strip_suffix(']')).unwrap_or(s)
}

/// A fully-resolved resolver choice (precedence resolution happens in callers).
#[derive(Debug, Clone)]
pub enum ResolverSpec {
    /// OS/system resolver (no configuration).
    System,
    /// Standard DNS against an explicit server list (`ip[:port]`).
    StandardDns { servers: Vec<String> },
    /// HTTP DNS via a configuration-prepared LinkSys generation.
    HttpDns(Arc<PreparedHttpDns>),
}

impl ResolverSpec {
    /// A stable fingerprint of this resolver choice, used as a data-plane cache
    /// key component so a resolver change (different servers, a swapped httpdns
    /// config) invalidates cached addresses instead of serving stale ones.
    ///
    /// HTTP DNS returns a precomputed admin-redacted address-cache identity.
    /// Secret bytes belong only to the private preparation identity; rotating
    /// them does not invalidate otherwise unchanged resolved backend addresses.
    pub fn fingerprint(&self) -> String {
        match self {
            ResolverSpec::System => "system".to_string(),
            ResolverSpec::StandardDns { servers } => format!("std:{}", servers.join(",")),
            ResolverSpec::HttpDns(prepared) => prepared.address_fingerprint.clone(),
        }
    }
}

/// Resolver failure classes (stable, low-cardinality).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsError {
    Io(String),
    Http(String),
    Parse(String),
    NoAddresses,
}

impl std::fmt::Display for DnsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DnsError::Io(s) => write!(f, "dns io error: {s}"),
            DnsError::Http(s) => write!(f, "http dns error: {s}"),
            DnsError::Parse(s) => write!(f, "http dns parse error: {s}"),
            DnsError::NoAddresses => write!(f, "no addresses resolved"),
        }
    }
}
impl std::error::Error for DnsError {}

/// Resolve `domain` per `spec`. Returns all resolved addresses.
pub async fn resolve(spec: &ResolverSpec, domain: &str) -> Result<Vec<IpAddr>, DnsError> {
    match spec {
        ResolverSpec::System => {
            let addrs = system::resolve_system(domain)
                .await
                .map_err(|e| DnsError::Io(e.to_string()))?;
            if addrs.is_empty() {
                Err(DnsError::NoAddresses)
            } else {
                Ok(addrs)
            }
        }
        ResolverSpec::StandardDns { servers } => standard::resolve_standard(servers, domain).await,
        ResolverSpec::HttpDns(prepared) => match prepared.fetcher.as_ref() {
            Some(fetcher) => httpdns::resolve_http_dns_with(fetcher, &prepared.config, domain).await,
            None => httpdns::resolve_via_fallback(&prepared.config, domain).await,
        },
    }
}

#[cfg(test)]
#[path = "../../tests/unit/dns/mod_test.rs"]
mod tests;
