//! System resolver backend (OS/libc via tokio::net::lookup_host).

use std::net::IpAddr;

/// Resolve a domain via the OS resolver, returning all addresses (deduped, order preserved).
pub async fn resolve_system(domain: &str) -> std::io::Result<Vec<IpAddr>> {
    // `domain` may already be a literal IP (including an unbracketed IPv6
    // literal like `::1`, which the `"{domain}:0"` trick below cannot parse
    // via `lookup_host` — it would be mis-split on the literal's own colons).
    if let Ok(ip) = domain.parse::<IpAddr>() {
        return Ok(vec![ip]);
    }
    // lookup_host requires a port; use a placeholder and discard it.
    let addr_str = format!("{}:0", domain);
    let mut out = Vec::new();
    for sa in tokio::net::lookup_host(&addr_str).await? {
        let ip = sa.ip();
        if !out.contains(&ip) {
            out.push(ip);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolves_localhost_to_loopback() {
        let addrs = resolve_system("localhost").await.expect("localhost resolves");
        assert!(
            addrs.iter().any(|ip| ip.is_loopback()),
            "expected a loopback, got {addrs:?}"
        );
    }

    #[tokio::test]
    async fn empty_host_errors() {
        assert!(resolve_system("").await.is_err());
    }

    #[tokio::test]
    async fn resolves_ipv6_literal_directly() {
        let addrs = resolve_system("::1").await.expect("ipv6 literal resolves");
        assert!(addrs.contains(&"::1".parse::<IpAddr>().unwrap()), "got {addrs:?}");
    }

    #[tokio::test]
    async fn resolves_ipv4_literal_directly() {
        let addrs = resolve_system("127.0.0.1").await.expect("ipv4 literal resolves");
        assert!(addrs.contains(&"127.0.0.1".parse::<IpAddr>().unwrap()), "got {addrs:?}");
    }
}
