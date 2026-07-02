//! Standard DNS backend using hickory-resolver against an explicit server list.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};

use super::{strip_brackets, system, DnsError};

/// Parse a `ip` or `ip:port` server string into a SocketAddr (default port 53).
/// Accepts a bracketed IPv6 literal without a port (e.g. `"[::1]"`).
fn parse_server(s: &str) -> Option<SocketAddr> {
    if let Ok(sa) = s.parse::<SocketAddr>() {
        return Some(sa);
    }
    let ip: IpAddr = strip_brackets(s).parse().ok()?;
    Some(SocketAddr::new(ip, 53))
}

/// Parse `servers` into distinct `SocketAddr`s, dropping any entry that fails
/// to parse. Pure and side-effect-free so the per-server-port grouping in
/// [`resolve_standard`] stays unit-testable without spinning up a resolver.
fn parse_servers(servers: &[String]) -> Vec<SocketAddr> {
    servers.iter().filter_map(|s| parse_server(s)).collect()
}

/// Resolve `domain` against the given DNS servers. Empty list ⇒ system resolver.
pub async fn resolve_standard(servers: &[String], domain: &str) -> Result<Vec<IpAddr>, DnsError> {
    if servers.is_empty() {
        let addrs = system::resolve_system(domain)
            .await
            .map_err(|e| DnsError::Io(e.to_string()))?;
        return if addrs.is_empty() {
            Err(DnsError::NoAddresses)
        } else {
            Ok(addrs)
        };
    }

    use hickory_resolver::config::{NameServerConfigGroup, ResolverConfig, ResolverOpts};
    use hickory_resolver::error::ResolveErrorKind;
    use hickory_resolver::TokioAsyncResolver;

    let socket_addrs = parse_servers(servers);
    if socket_addrs.is_empty() {
        return Err(DnsError::Io(format!("no valid DNS servers in {servers:?}")));
    }

    // Group IPs by their configured port so each server keeps its own port
    // instead of every server inheriting the first server's port.
    let mut ips_by_port: BTreeMap<u16, Vec<IpAddr>> = BTreeMap::new();
    for sa in &socket_addrs {
        ips_by_port.entry(sa.port()).or_default().push(sa.ip());
    }
    let mut group = NameServerConfigGroup::with_capacity(socket_addrs.len() * 2);
    for (port, ips) in ips_by_port {
        group.merge(NameServerConfigGroup::from_ips_clear(&ips, port, true));
    }
    let config = ResolverConfig::from_parts(None, vec![], group);
    let resolver = TokioAsyncResolver::tokio(config, ResolverOpts::default());

    let lookup = resolver.lookup_ip(domain).await.map_err(|e| {
        if matches!(e.kind(), ResolveErrorKind::NoRecordsFound { .. }) {
            DnsError::NoAddresses
        } else {
            DnsError::Io(e.to_string())
        }
    })?;
    let out: Vec<IpAddr> = lookup.iter().collect();
    if out.is_empty() {
        Err(DnsError::NoAddresses)
    } else {
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_servers_falls_back_to_system() {
        // No servers configured ⇒ behave exactly like the system resolver.
        let addrs = resolve_standard(&[], "localhost")
            .await
            .expect("system fallback resolves");
        assert!(addrs.iter().any(|ip| ip.is_loopback()));
    }

    #[test]
    fn parses_server_with_and_without_port() {
        assert_eq!(
            parse_server("1.1.1.1").unwrap(),
            "1.1.1.1:53".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            parse_server("1.1.1.1:5353").unwrap(),
            "1.1.1.1:5353".parse::<SocketAddr>().unwrap()
        );
        assert!(parse_server("not an ip").is_none());
    }

    #[test]
    fn parses_bracketed_ipv6_without_port() {
        assert_eq!(
            parse_server("[::1]").unwrap(),
            SocketAddr::new("::1".parse::<IpAddr>().unwrap(), 53)
        );
        assert_eq!(
            parse_server("[2001:db8::1]").unwrap(),
            SocketAddr::new("2001:db8::1".parse::<IpAddr>().unwrap(), 53)
        );
    }

    // DR-1: each server must keep its own configured port instead of every
    // server inheriting the first server's port.
    #[test]
    fn parse_servers_preserves_per_server_ports() {
        let servers = vec!["1.1.1.1:5353".to_string(), "8.8.8.8:9999".to_string()];
        let addrs = parse_servers(&servers);
        assert_eq!(addrs.len(), 2);
        assert!(addrs.contains(&"1.1.1.1:5353".parse::<SocketAddr>().unwrap()));
        assert!(addrs.contains(&"8.8.8.8:9999".parse::<SocketAddr>().unwrap()));
    }
}
