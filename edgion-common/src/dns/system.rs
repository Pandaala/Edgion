//! System resolver backend (OS/libc on Tokio's blocking pool).

use super::DnsQueryPolicy;
use std::net::{IpAddr, ToSocketAddrs};
use std::time::Duration;
use tokio_util::task::AbortOnDropHandle;

/// Deadline for one OS resolver lookup. `getaddrinfo` has no caller deadline of
/// its own (only `resolv.conf` timeouts multiplied by search-list expansion),
/// and a cached lookup holds its per-key flight while it runs, so the wait is
/// bounded here. The blocking `getaddrinfo` call itself cannot be cancelled and
/// finishes on its blocking-pool thread; the bound is generous so a slow but
/// successful lookup (Kubernetes `ndots` expansion) is not turned into a failure.
const SYSTEM_RESOLVE_TIMEOUT: Duration = Duration::from_secs(30);

/// Keep the OS socket addresses intact for transports that dial scoped IPv6.
pub(crate) async fn lookup_system_sockets<A>(address: A) -> std::io::Result<A::Iter>
where
    A: ToSocketAddrs + Send + 'static,
    A::Iter: Send + 'static,
{
    // Cancellation or timeout aborts a queued job. A libc lookup already in
    // progress finishes on its blocking thread.
    let lookup = AbortOnDropHandle::new(tokio::task::spawn_blocking(move || address.to_socket_addrs()));
    tokio::time::timeout(SYSTEM_RESOLVE_TIMEOUT, lookup)
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "system DNS lookup timed out"))??
}

/// Resolve a domain via the OS resolver, returning all addresses (deduped, order preserved).
pub async fn resolve_system(domain: &str, policy: DnsQueryPolicy) -> std::io::Result<Vec<IpAddr>> {
    // Literal addresses do not require an OS lookup.
    if let Ok(ip) = domain.parse::<IpAddr>() {
        return Ok(policy.select(vec![ip]));
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for sa in lookup_system_sockets(format!("{domain}:0")).await? {
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
