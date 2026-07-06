//! Shared DNS resolver used by both the controller (background resolution)
//! and the gateway (data-plane cache). Backend-agnostic; the standard-DNS
//! backend uses hickory-resolver, HTTP DNS reuses link_sys_core's HTTP client,
//! and the no-config path falls back to the system resolver.

pub mod http_fetcher;
pub mod httpdns;
pub mod resolver_select;
pub mod standard;
pub mod system;

use std::net::IpAddr;

use edgion_resources::resources::link_sys::HttpDnsResolverConfig;

/// Strip a single pair of surrounding `[` `]` brackets, if present (bracketed
/// IPv6 literal without a port, e.g. `"[::1]"`). Shared by the standard-DNS
/// server-string parser and the HTTP DNS response-body IP parser so a
/// bracketed IPv6 literal parses the same way in both places.
pub(crate) fn strip_brackets(s: &str) -> &str {
    s.strip_prefix('[').and_then(|s| s.strip_suffix(']')).unwrap_or(s)
}

/// A fully-resolved resolver choice (precedence resolution happens in callers).
// No `PartialEq`/`Eq`: the `HttpDns` arm wraps `HttpDnsResolverConfig`, which has neither.
#[derive(Debug, Clone)]
pub enum ResolverSpec {
    /// OS/system resolver (no configuration).
    System,
    /// Standard DNS against an explicit server list (`ip[:port]`).
    StandardDns { servers: Vec<String> },
    /// HTTP DNS via a LinkSys httpdns config.
    ///
    /// Boxed: `HttpDnsResolverConfig` is much larger than the other variants, so
    /// carrying it inline triggers `clippy::large_enum_variant`.
    HttpDns(Box<HttpDnsResolverConfig>),
}

impl ResolverSpec {
    /// A stable fingerprint of this resolver choice, used as a data-plane cache
    /// key component so a resolver change (different servers, a swapped httpdns
    /// config) invalidates cached addresses instead of serving stale ones.
    ///
    /// `System` and `StandardDns` fingerprint from their fields directly; the
    /// `HttpDns` variant serializes its config (order-stable serde) because it
    /// has no cheaper structural identity.
    pub fn fingerprint(&self) -> String {
        match self {
            ResolverSpec::System => "system".to_string(),
            ResolverSpec::StandardDns { servers } => format!("std:{}", servers.join(",")),
            // serde_json is order-stable for struct fields; a serialize failure
            // (not expected for this plain config) degrades to a constant tag,
            // which only means "do not distinguish two httpdns configs" — never
            // a correctness hazard because the resolved addresses are re-validated.
            //
            // Serialize under an admin-redact guard so any resolved secret
            // material in `connection.tls` (client key / CA PEMs) collapses to
            // "[redacted]" and never enters the process-global cache key. A
            // secret-only change (e.g. a rotated client cert) does not alter the
            // resolved IPs, so redacting it from the key is also semantically
            // correct — it must not force a cache invalidation.
            ResolverSpec::HttpDns(cfg) => {
                let _redact = edgion_resources::redact::AdminRedactGuard::new();
                format!("httpdns:{}", serde_json::to_string(cfg).unwrap_or_default())
            }
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
        ResolverSpec::HttpDns(cfg) => {
            // Build the real network fetcher from the resolver's connection
            // settings (outbound TLS + timeout) and run the transport-free
            // render/parse/fallback core against it. If the client cannot even be
            // built (e.g. invalid resolved TLS material), fall through to the
            // configured fallback rather than dropping it.
            match http_fetcher::ReqwestHttpDnsFetcher::new(cfg.connection.as_ref()) {
                Ok(fetcher) => httpdns::resolve_http_dns_with(&fetcher, cfg, domain).await,
                Err(_build_err) => httpdns::resolve_via_fallback(cfg, domain).await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn system_spec_resolves_localhost() {
        let addrs = resolve(&ResolverSpec::System, "localhost").await.expect("resolves");
        assert!(addrs.iter().any(|ip| ip.is_loopback()));
    }

    #[test]
    fn fingerprint_distinguishes_resolvers() {
        let system = ResolverSpec::System.fingerprint();
        let dns_a = ResolverSpec::StandardDns {
            servers: vec!["1.1.1.1".to_string()],
        }
        .fingerprint();
        let dns_b = ResolverSpec::StandardDns {
            servers: vec!["8.8.8.8".to_string()],
        }
        .fingerprint();
        assert_ne!(system, dns_a);
        assert_ne!(dns_a, dns_b, "different server lists must fingerprint differently");
        // Same servers → same fingerprint (cache hit across requests).
        assert_eq!(
            dns_a,
            ResolverSpec::StandardDns {
                servers: vec!["1.1.1.1".to_string()]
            }
            .fingerprint()
        );
    }

    #[test]
    fn httpdns_fingerprint_redacts_resolved_secret_material() {
        use edgion_resources::resources::common::{OutboundTlsLocal, TlsConfig};
        use edgion_resources::resources::link_sys::httpdns::{
            HttpDnsConnection, HttpDnsFallback, HttpDnsPreset, HttpDnsResolverConfig, HttpDnsResponseFormat,
        };
        use k8s_openapi::api::core::v1::Secret;
        use std::collections::BTreeMap;

        const SENTINEL: &str = "SUPER_SECRET_KEY_MATERIAL";
        let mut string_data = BTreeMap::new();
        string_data.insert("tls.key".to_string(), SENTINEL.to_string());
        let secret = Secret {
            string_data: Some(string_data),
            ..Default::default()
        };
        let tls = TlsConfig {
            enabled: true,
            outbound: OutboundTlsLocal::default(),
            resolved_ca_certificates: None,
            resolved_client_certificate: Some(secret),
        };
        let cfg = HttpDnsResolverConfig {
            preset: Some(HttpDnsPreset::Aliyun),
            url_template: None,
            response: HttpDnsResponseFormat::default(),
            fallback: HttpDnsFallback::System,
            connection: Some(HttpDnsConnection {
                tls: Some(tls),
                timeout_ms: None,
            }),
        };
        let fp = ResolverSpec::HttpDns(Box::new(cfg)).fingerprint();
        assert!(
            !fp.contains(SENTINEL),
            "resolved secret material must not appear in the cache fingerprint: {fp}"
        );
        assert!(
            fp.contains("[redacted]"),
            "expected redaction marker in fingerprint: {fp}"
        );
    }

    #[test]
    fn strip_brackets_removes_matching_pair_only() {
        assert_eq!(strip_brackets("[::1]"), "::1");
        assert_eq!(strip_brackets("[2001:db8::1]"), "2001:db8::1");
        assert_eq!(strip_brackets("::1"), "::1");
        assert_eq!(strip_brackets("[::1"), "[::1");
    }
}
