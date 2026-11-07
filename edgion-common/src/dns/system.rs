//! System resolver backend (OS/libc via tokio::net::lookup_host).

use super::DnsQueryPolicy;
use std::net::IpAddr;
use std::time::Duration;

/// Deadline for one OS resolver lookup. `getaddrinfo` has no caller deadline of
/// its own (only `resolv.conf` timeouts multiplied by search-list expansion),
/// and a cached lookup holds its per-key flight while it runs, so the wait is
/// bounded here. The blocking `getaddrinfo` call itself cannot be cancelled and
/// finishes on its blocking-pool thread; the bound is generous so a slow but
/// successful lookup (Kubernetes `ndots` expansion) is not turned into a failure.
const SYSTEM_RESOLVE_TIMEOUT: Duration = Duration::from_secs(30);

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
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let resolved = tokio::time::timeout(SYSTEM_RESOLVE_TIMEOUT, tokio::net::lookup_host(&addr_str))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "system DNS lookup timed out"))??;
    for sa in resolved {
        let ip = sa.ip();
        // Linear dedup; the shared answer cap applies after family selection.
        if seen.insert(ip) {
            out.push(ip);
        }
    }
    Ok(policy.select(out))
}

#[cfg(test)]
#[path = "../../tests/unit/dns/system_test.rs"]
mod tests;
