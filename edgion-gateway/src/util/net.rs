use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::str::FromStr;

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
/// services (AWS/GCP/Azure IMDS at `169.254.169.254`, plus the broader link-local segment).
///
/// Deliberately **not** blocked:
/// - RFC1918 private ranges (10/8, 172.16/12, 192.168/16) — common K8s cluster traffic
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
        IpClass::Private | IpClass::UniqueLocal | IpClass::Unspecified | IpClass::Global => None,
    }
}

/// Resolve domain to SocketAddr using async DNS resolution.
///
/// Uses tokio's async DNS resolver to avoid blocking the event loop.
/// Returns the first resolved address.
pub async fn resolve_domain(domain: &str, port: u16) -> pingora_core::Result<SocketAddr> {
    // Short-circuit IP literals: build the SocketAddr directly instead of handing
    // lookup_host an unbracketed "{ip}:{port}". For an IPv6 literal that string (e.g.
    // "::1:9090") is itself a valid IPv6 address and only resolves via lookup_host's
    // last-colon host:port heuristic — fragile and ambiguous. Parsing the literal here
    // is RFC-correct and also avoids a needless resolver round-trip.
    if let Some(ip) = parse_ip_literal(domain) {
        return Ok(SocketAddr::new(ip, port));
    }

    let mut addrs = tokio::net::lookup_host((domain, port)).await.map_err(|e| {
        PingoraError::explain(
            ErrorType::ConnectError,
            format!("DNS resolution failed for {}: {}", domain, e),
        )
    })?;

    addrs
        .next()
        .ok_or_else(|| PingoraError::explain(ErrorType::ConnectError, format!("No addresses found for {}", domain)))
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
