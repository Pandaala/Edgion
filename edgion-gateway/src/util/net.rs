use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::str::FromStr;

use edgion_common::dns::{DnsCachePolicy, DnsLookupFamily, DnsQueryPolicy, ResolverSpec};
pub(crate) use edgion_common::egress::{classify_ip, IpClass};
use pingora_core::protocols::l4::socket::SocketAddr as PingoraSocketAddr;
use pingora_core::{Error as PingoraError, ErrorType};

/// Category of backend address blocked for security reasons.
///
/// Stable, low-cardinality labels suitable for request diagnostics / access log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockedBackendReason {
    /// 127.0.0.0/8 or ::1 — loopback
    Localhost,
    /// 169.254.0.0/16 (incl. cloud metadata 169.254.169.254) or fe80::/10
    LinkLocal,
    /// Operator-configured `outbound_egress_blocked_cidrs` (e.g. vendor cloud
    /// metadata endpoints outside link-local)
    BlockedCidr,
    /// Address unsuitable for a unicast dial, including invalid transition endpoints.
    NonDialable,
}

fn parse_ip_literal(domain: &str) -> Option<IpAddr> {
    IpAddr::from_str(domain).ok().or_else(|| {
        domain
            .strip_prefix('[')
            .and_then(|inner| inner.strip_suffix(']'))
            .and_then(|inner| Ipv6Addr::from_str(inner).ok())
            .map(IpAddr::V6)
    })
}

/// Classify whether a backend address should be rejected before dialing.
///
/// Returns `Some(reason)` if the address falls into a blocked category, `None` if allowed.
///
/// Blocked categories protect against SSRF to host-local loopback and cloud metadata
/// services (AWS/GCP/Azure IMDS at `169.254.169.254`, plus the broader link-local segment),
/// as well as non-dialable destinations and operator-configured blocked CIDRs.
///
/// Deliberately **not** blocked:
/// - RFC1918 and CGNAT ranges used for cluster traffic
/// - Unix domain sockets — sidecar/local IPC pattern
///
/// Note: this is defense-in-depth. The primary egress control should be a NetworkPolicy
/// on the Gateway Pod blocking `169.254.0.0/16` at the kernel level.
#[inline]
pub fn classify_backend_addr(addr: &PingoraSocketAddr) -> Option<BlockedBackendReason> {
    // Non-inet upstreams (Unix domain sockets — the sidecar/local-IPC pattern)
    // are not classified and remain allowed via this `?` short-circuit.
    let ip = addr.as_inet()?.ip();
    // Exhaustive match, no wildcard: a new IpClass variant becomes a compile error
    // here, forcing a conscious block/allow decision. Backend deliberately ALLOWS
    // Private / UniqueLocal / Unspecified (cluster-internal pod/service traffic).
    match classify_ip(ip) {
        IpClass::Loopback => Some(BlockedBackendReason::Localhost),
        IpClass::LinkLocal => Some(BlockedBackendReason::LinkLocal),
        IpClass::BlockedCidr => Some(BlockedBackendReason::BlockedCidr),
        IpClass::NonDialable => Some(BlockedBackendReason::NonDialable),
        IpClass::Private | IpClass::UniqueLocal | IpClass::Unspecified | IpClass::Global => None,
    }
}

/// Resolve an external destination domain to the addresses to try, in order.
///
/// IP literals are returned as-is. Names go through the shared upstream DNS
/// answer cache with the caller's resolver, address family and cache policy
/// (the same path as AI provider endpoints), and the cached answer is rotated
/// by the process-wide offset so traffic spreads over every address.
pub async fn resolve_domain(
    domain: &str,
    port: u16,
    spec: &ResolverSpec,
    family: DnsLookupFamily,
    policy: &DnsCachePolicy,
) -> pingora_core::Result<Vec<SocketAddr>> {
    // Short-circuit IP literals: build the SocketAddr directly. A bare IPv6
    // literal such as "::1" must never be re-split on a colon, and a literal
    // keeps its own family whatever the configured lookup family is.
    if let Some(ip) = parse_ip_literal(domain) {
        return Ok(vec![SocketAddr::new(ip, port)]);
    }

    let mut ips = crate::backends::upstream_dns_cache()
        .lookup(spec, domain, DnsQueryPolicy::Family(family), policy)
        .await
        .map_err(|e| {
            PingoraError::explain(
                ErrorType::ConnectError,
                format!("DNS resolution failed for {}: {}", domain, e),
            )
        })?;
    crate::backends::rotate_addresses(&mut ips, crate::backends::next_address_rotation());
    Ok(ips.into_iter().map(|ip| SocketAddr::new(ip, port)).collect())
}

/// Strip the optional `:port` suffix from a Host-header authority, preserving an
/// IPv6 literal's surrounding brackets so the result can be re-emitted into a URL.
///
/// Unlike `NormalizedHostname`, this keeps the brackets (`[::1]:8443` → `[::1]`, not
/// `::1`) and does not lowercase or strip a trailing FQDN dot — it is meant for
/// echoing the client's host back into a `Location` redirect target, where dropping
/// the brackets would produce an RFC-3986-invalid authority (`https://::1/`).
///
/// - `[::1]:8443` → `[::1]`     (IPv6 literal with port)
/// - `[::1]`      → `[::1]`     (IPv6 literal, no port)
/// - `example.com:80` → `example.com`
/// - `example.com` → `example.com`
/// - `[::1`       → `[::1`      (malformed: returned unchanged)
/// - `2001:db8::1` → `2001:db8::1` (bare/unbracketed IPv6: returned unchanged)
pub fn host_without_port(host: &str) -> &str {
    if host.starts_with('[') {
        // IPv6 literal: keep everything up to and including the closing ']'.
        match host.find(']') {
            Some(end) => &host[..=end],
            None => host,
        }
    } else {
        // IPv4 / reg-name: strip a single trailing `:port` (digits only).
        // With more than one colon, `suffix` contains a colon and fails the
        // digit-only check, leaving bare IPv6 literals unchanged.
        match host.split_once(':') {
            Some((name, suffix)) if suffix.bytes().all(|byte| byte.is_ascii_digit()) => name,
            _ => host,
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/util/net_test.rs"]
mod tests;
