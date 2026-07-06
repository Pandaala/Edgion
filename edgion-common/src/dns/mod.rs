//! Shared DNS resolver used by both the controller (background resolution)
//! and the gateway (data-plane cache). Backend-agnostic; the standard-DNS
//! backend uses hickory-resolver, HTTP DNS reuses link_sys_core's HTTP client,
//! and the no-config path falls back to the system resolver.

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
        // Deferred to Part B: this dispatch needs the real HTTP fetcher (built on
        // link_sys_core's HTTP client). The render/parse/fallback core already
        // exists in `httpdns::resolve_http_dns_with` and is unit-tested; only the
        // network fetch + connection/auth wiring remains.
        ResolverSpec::HttpDns(_) => Err(DnsError::Http("http dns not yet implemented".into())),
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
    fn strip_brackets_removes_matching_pair_only() {
        assert_eq!(strip_brackets("[::1]"), "::1");
        assert_eq!(strip_brackets("[2001:db8::1]"), "2001:db8::1");
        assert_eq!(strip_brackets("::1"), "::1");
        assert_eq!(strip_brackets("[::1"), "[::1");
    }
}
