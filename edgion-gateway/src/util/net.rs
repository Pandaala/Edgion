use pingora_core::protocols::l4::socket::SocketAddr as PingoraSocketAddr;
use pingora_core::{Error as PingoraError, ErrorType};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::str::FromStr;

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

/// Coarse classification of an IP address into the segments that matter for
/// SSRF / egress policy. Low-cardinality and **policy-free**: each caller maps
/// these classes to its own allow/block decision — see [`classify_backend_addr`]
/// for the backend policy (allows private) and the DSL `is_ssrf_blocked_ip_literal`
/// for the DSL policy (blocks private).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IpClass {
    /// 127.0.0.0/8 or ::1
    Loopback,
    /// 169.254.0.0/16 (incl. cloud metadata 169.254.169.254) or fe80::/10
    LinkLocal,
    /// RFC1918 private (10/8, 172.16/12, 192.168/16)
    Private,
    /// RFC 4193 unique-local (fc00::/7)
    UniqueLocal,
    /// 0.0.0.0 or ::
    Unspecified,
    /// Anything else — globally routable / public.
    Global,
}

/// Classify an IP address into a single [`IpClass`] segment.
///
/// This is the single source of truth for IP-segment classification shared by the
/// backend validator and the DSL SSRF guard. It does **not** decide allow/block —
/// callers map the returned class to their own policy.
///
/// IPv4-mapped IPv6 (`::ffff:a.b.c.d`) is normalized to its embedded IPv4 and
/// classified through the IPv4 rules. This normalization **must run before** the
/// IPv6 segment-mask checks: `::ffff:127.0.0.1` is neither `::1` nor inside
/// `fc00`/`fe80`, so without the early mapping it would fall through to `Global` —
/// a silent SSRF bypass. Note `::` is not a mapped address (`to_ipv4_mapped()`
/// returns `None` for it), so it correctly reaches the unspecified arm.
#[inline]
pub(crate) fn classify_ip(ip: IpAddr) -> IpClass {
    match ip {
        IpAddr::V4(v4) => classify_v4_class(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                classify_v4_class(v4)
            } else if v6.is_loopback() {
                IpClass::Loopback
            } else if v6.is_unspecified() {
                IpClass::Unspecified
            } else {
                let s0 = v6.segments()[0];
                if (s0 & 0xfe00) == 0xfc00 {
                    IpClass::UniqueLocal
                } else if (s0 & 0xffc0) == 0xfe80 {
                    IpClass::LinkLocal
                } else {
                    IpClass::Global
                }
            }
        }
    }
}

/// Classify an IPv4 address into an [`IpClass`]. Shared by the `IpAddr::V4` branch
/// and the IPv4-mapped IPv6 path of [`classify_ip`]. The segments are disjoint, so
/// the check order documents intent rather than resolving overlaps.
#[inline]
fn classify_v4_class(ip: Ipv4Addr) -> IpClass {
    if ip.is_loopback() {
        IpClass::Loopback
    } else if ip.is_link_local() {
        IpClass::LinkLocal
    } else if ip.is_private() {
        IpClass::Private
    } else if ip.is_unspecified() {
        IpClass::Unspecified
    } else {
        IpClass::Global
    }
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
    if let Ok(ip) = IpAddr::from_str(domain) {
        return Ok(SocketAddr::new(ip, port));
    }

    let addr_str = format!("{}:{}", domain, port);

    let mut addrs = tokio::net::lookup_host(&addr_str).await.map_err(|e| {
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
        // IPv4 / reg-name: strip a single trailing `:port` (digits only). The
        // `count() == 1` guard rejects a bare (unbracketed) IPv6 literal such as
        // `2001:db8::1` — which has >1 colon and no valid port to strip — so it is
        // returned unchanged instead of being mangled into `2001:db8:`.
        match host.rfind(':') {
            Some(pos) if host.matches(':').count() == 1 && host[pos + 1..].bytes().all(|b| b.is_ascii_digit()) => {
                &host[..pos]
            }
            _ => host,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_host_without_port() {
        // IPv6 literals keep their brackets (port stripped or not).
        assert_eq!(host_without_port("[::1]:8443"), "[::1]");
        assert_eq!(host_without_port("[::1]"), "[::1]");
        assert_eq!(host_without_port("[2001:db8::1]:443"), "[2001:db8::1]");
        // IPv4 / registered names: strip a single trailing :port.
        assert_eq!(host_without_port("example.com:80"), "example.com");
        assert_eq!(host_without_port("example.com"), "example.com");
        assert_eq!(host_without_port("127.0.0.1:8080"), "127.0.0.1");
        // Non-numeric suffix after ':' is not a port, so it is preserved as-is.
        assert_eq!(host_without_port("host:notaport"), "host:notaport");
        // Malformed IPv6 (no closing bracket) is returned unchanged.
        assert_eq!(host_without_port("[::1"), "[::1");
        // Bare/unbracketed IPv6 literals have >1 colon and no strippable port; the
        // single-colon guard must leave them untouched (not mangle to `2001:db8:`).
        assert_eq!(host_without_port("2001:db8::1"), "2001:db8::1");
        assert_eq!(host_without_port("::1"), "::1");
    }

    #[test]
    fn test_classify_backend_addr_link_local_metadata() {
        // AWS/GCP/Azure cloud metadata endpoint
        let addr = "169.254.169.254:80".parse::<PingoraSocketAddr>().unwrap();
        assert_eq!(
            classify_backend_addr(&addr),
            Some(BlockedBackendReason::LinkLocal),
            "169.254.169.254 (cloud metadata) must be blocked"
        );

        // Broader link-local segment
        let addr = "169.254.0.1:80".parse::<PingoraSocketAddr>().unwrap();
        assert_eq!(classify_backend_addr(&addr), Some(BlockedBackendReason::LinkLocal));
        let addr = "169.254.255.254:80".parse::<PingoraSocketAddr>().unwrap();
        assert_eq!(classify_backend_addr(&addr), Some(BlockedBackendReason::LinkLocal));
    }

    #[test]
    fn test_classify_backend_addr_link_local_ipv6() {
        // fe80::/10 covers fe80::…fe bf::…; check both ends of the range
        let addr = "[fe80::1]:80".parse::<PingoraSocketAddr>().unwrap();
        assert_eq!(classify_backend_addr(&addr), Some(BlockedBackendReason::LinkLocal));

        let addr = "[febf::1]:80".parse::<PingoraSocketAddr>().unwrap();
        assert_eq!(classify_backend_addr(&addr), Some(BlockedBackendReason::LinkLocal));

        // fec0:: is outside fe80::/10 — not link-local (historical site-local, deprecated, but not blocked)
        let addr = "[fec0::1]:80".parse::<PingoraSocketAddr>().unwrap();
        assert_eq!(classify_backend_addr(&addr), None);
    }

    #[test]
    fn test_classify_backend_addr_localhost() {
        let addr = "127.0.0.1:80".parse::<PingoraSocketAddr>().unwrap();
        assert_eq!(classify_backend_addr(&addr), Some(BlockedBackendReason::Localhost));
        let addr = "[::1]:80".parse::<PingoraSocketAddr>().unwrap();
        assert_eq!(classify_backend_addr(&addr), Some(BlockedBackendReason::Localhost));
    }

    #[test]
    fn test_classify_backend_addr_rfc1918_allowed() {
        // RFC1918 private ranges must NOT be blocked — they are common cluster-internal backends
        for candidate in [
            "10.0.0.1:80",
            "10.255.255.255:80",
            "172.16.0.1:80",
            "172.31.255.254:80",
            "192.168.1.1:80",
        ] {
            let addr = candidate.parse::<PingoraSocketAddr>().unwrap();
            assert_eq!(
                classify_backend_addr(&addr),
                None,
                "{candidate} must be allowed (RFC1918)"
            );
        }
    }

    #[test]
    fn test_classify_backend_addr_public_allowed() {
        let addr = "8.8.8.8:80".parse::<PingoraSocketAddr>().unwrap();
        assert_eq!(classify_backend_addr(&addr), None);
        let addr = "[2001:db8::1]:80".parse::<PingoraSocketAddr>().unwrap();
        assert_eq!(classify_backend_addr(&addr), None);
    }

    #[test]
    fn test_classify_backend_addr_ipv4_mapped_ipv6() {
        // IPv4-mapped IPv6 (`::ffff:a.b.c.d`) must be re-classified via the embedded IPv4
        // address; otherwise loopback / link-local targets bypass the guard (F-003).
        let addr = "[::ffff:127.0.0.1]:80".parse::<PingoraSocketAddr>().unwrap();
        assert_eq!(
            classify_backend_addr(&addr),
            Some(BlockedBackendReason::Localhost),
            "::ffff:127.0.0.1 (mapped loopback) must be blocked"
        );

        let addr = "[::ffff:169.254.169.254]:80".parse::<PingoraSocketAddr>().unwrap();
        assert_eq!(
            classify_backend_addr(&addr),
            Some(BlockedBackendReason::LinkLocal),
            "::ffff:169.254.169.254 (mapped cloud metadata) must be blocked"
        );

        // A mapped *public* address must still be allowed — the normalization must not
        // over-block legitimate dual-stack backends.
        let addr = "[::ffff:8.8.8.8]:80".parse::<PingoraSocketAddr>().unwrap();
        assert_eq!(
            classify_backend_addr(&addr),
            None,
            "::ffff:8.8.8.8 (mapped public) must be allowed"
        );

        // Plain `::1` is NOT an IPv4-mapped address (`to_ipv4_mapped()` returns None for it),
        // so it must still be caught by the IPv6 loopback check, not silently downgraded.
        let addr = "[::1]:80".parse::<PingoraSocketAddr>().unwrap();
        assert_eq!(classify_backend_addr(&addr), Some(BlockedBackendReason::Localhost));
    }

    // resolve_domain tests — kept hermetic by using IP literals and localhost only,
    // so they don't depend on a working external resolver.

    #[tokio::test]
    async fn resolve_domain_ipv4_literal() {
        let addr = resolve_domain("127.0.0.1", 8080).await.expect("ipv4 literal resolves");
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        assert_eq!(addr.port(), 8080);
    }

    #[tokio::test]
    async fn resolve_domain_ipv6_literal() {
        // Hardened path: IPv6 literals are short-circuited through IpAddr::from_str and never
        // handed to lookup_host as an unbracketed "::1:9090" (which is itself a valid IPv6
        // address and would otherwise rely on resolver last-colon leniency).
        let addr = resolve_domain("::1", 9090).await.expect("ipv6 literal resolves");
        assert_eq!(addr.ip().to_string(), "::1");
        assert_eq!(addr.port(), 9090);

        // A multi-group literal is the ambiguous case the short-circuit guards against:
        // it must resolve to exactly the given address and port, not be re-split on a colon.
        let addr = resolve_domain("2001:db8::1", 8443)
            .await
            .expect("multi-group ipv6 literal resolves");
        assert_eq!(addr.ip().to_string(), "2001:db8::1");
        assert_eq!(addr.port(), 8443);
    }

    #[tokio::test]
    async fn resolve_domain_localhost() {
        let addr = resolve_domain("localhost", 80).await.expect("localhost resolves");
        assert!(addr.ip().is_loopback(), "localhost must resolve to a loopback address");
        assert_eq!(addr.port(), 80);
    }

    #[tokio::test]
    async fn resolve_domain_invalid_returns_connect_error() {
        // Empty host string is rejected at the parse stage by lookup_host before any
        // resolver is consulted, so this stays hermetic even on networks where the
        // ISP/DNS hijacks NXDOMAIN responses (RFC 2606 .invalid is unreliable in
        // practice for that reason).
        let err = resolve_domain("", 80).await.expect_err("must fail for invalid host");
        assert_eq!(err.etype(), &ErrorType::ConnectError);
    }

    #[test]
    fn test_classify_ip_segments() {
        use std::net::IpAddr;
        let cases: &[(&str, IpClass)] = &[
            // Loopback
            ("127.0.0.1", IpClass::Loopback),
            ("127.255.255.255", IpClass::Loopback),
            ("::1", IpClass::Loopback),
            // Link-local (incl. cloud metadata) + fe80::/10 boundaries
            ("169.254.0.1", IpClass::LinkLocal),
            ("169.254.169.254", IpClass::LinkLocal),
            ("fe80::1", IpClass::LinkLocal),
            ("febf::1", IpClass::LinkLocal),
            ("fec0::1", IpClass::Global), // just outside fe80::/10
            // RFC1918 private
            ("10.0.0.1", IpClass::Private),
            ("172.16.0.1", IpClass::Private),
            ("172.31.255.255", IpClass::Private),
            ("192.168.1.1", IpClass::Private),
            // Unique-local fc00::/7 (both ends)
            ("fc00::1", IpClass::UniqueLocal),
            ("fdff::1", IpClass::UniqueLocal),
            ("fd12:3456::1", IpClass::UniqueLocal),
            // Unspecified
            ("0.0.0.0", IpClass::Unspecified),
            ("::", IpClass::Unspecified),
            // Global / public
            ("8.8.8.8", IpClass::Global),
            ("2001:db8::1", IpClass::Global),
            ("2606:4700::1111", IpClass::Global),
        ];
        for (s, want) in cases {
            let ip: IpAddr = s.parse().unwrap();
            assert_eq!(classify_ip(ip), *want, "classify_ip({s})");
        }
    }

    #[test]
    fn test_classify_ip_ipv4_mapped_normalized_first() {
        use std::net::IpAddr;
        // IPv4-mapped IPv6 must be re-classified via the embedded IPv4 address.
        // Without normalizing FIRST, ::ffff:127.0.0.1 would fall through to Global.
        let mapped: &[(&str, IpClass)] = &[
            ("::ffff:127.0.0.1", IpClass::Loopback),
            ("::ffff:169.254.169.254", IpClass::LinkLocal),
            ("::ffff:192.168.1.1", IpClass::Private),
            ("::ffff:0.0.0.0", IpClass::Unspecified),
            ("::ffff:8.8.8.8", IpClass::Global),
        ];
        for (s, want) in mapped {
            let ip: IpAddr = s.parse().unwrap();
            assert_eq!(classify_ip(ip), *want, "mapped classify_ip({s})");
        }
        // `::` must NOT be mistaken for a mapped address (to_ipv4_mapped() == None),
        // so it reaches the unspecified arm rather than being classified as v4.
        assert_eq!(classify_ip("::".parse::<IpAddr>().unwrap()), IpClass::Unspecified);
    }
}
