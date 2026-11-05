//! System resolver backend (OS/libc via tokio::net::lookup_host).

use super::DnsQueryPolicy;
use std::net::IpAddr;

/// Resolve a domain via the OS resolver, returning all addresses (deduped, order preserved).
pub async fn resolve_system(domain: &str, policy: DnsQueryPolicy) -> std::io::Result<Vec<IpAddr>> {
    // `domain` may already be a literal IP (including an unbracketed IPv6
    // literal like `::1`, which the `"{domain}:0"` trick below cannot parse
    // via `lookup_host` — it would be mis-split on the literal's own colons).
    if let Ok(ip) = domain.parse::<IpAddr>() {
        return Ok(policy.select(vec![ip]));
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
    Ok(policy.select(out))
}

#[cfg(test)]
#[path = "../../tests/unit/dns/system_test.rs"]
mod tests;
