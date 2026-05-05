use anyhow::{Context, Result};
use pingora_core::protocols::l4::socket::SocketAddr as PingoraSocketAddr;
use pingora_core::{Error as PingoraError, ErrorType};
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;

pub const DEFAULT_OPERATOR_GRPC_ADDR: &str = "127.0.0.1:50061";

pub fn parse_listen_addr(addr: Option<&String>, default: &str) -> Result<SocketAddr> {
    let candidate = addr.map(String::as_str).unwrap_or(default);
    SocketAddr::from_str(candidate).with_context(|| format!("failed to parse listen address '{}'", candidate))
}

pub fn parse_optional_listen_addr(addr: Option<&String>) -> Result<Option<SocketAddr>> {
    addr.map(|value| SocketAddr::from_str(value).with_context(|| format!("failed to parse listen address '{}'", value)))
        .transpose()
}

pub fn normalize_grpc_endpoint(addr: &str) -> String {
    if addr.starts_with("http://") || addr.starts_with("https://") {
        addr.to_string()
    } else {
        format!("http://{}", addr)
    }
}

pub fn default_operator_addr() -> &'static str {
    DEFAULT_OPERATOR_GRPC_ADDR
}

/// Category of backend address blocked for security reasons.
///
/// Stable, low-cardinality labels suitable for `edgion_status` / access log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockedBackendReason {
    /// 127.0.0.0/8 or ::1 — loopback
    Localhost,
    /// 169.254.0.0/16 (incl. cloud metadata 169.254.169.254) or fe80::/10
    LinkLocal,
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
    let inet_addr = addr.as_inet()?;
    match inet_addr.ip() {
        IpAddr::V4(ip) => {
            if ip.octets()[0] == 127 {
                Some(BlockedBackendReason::Localhost)
            } else if ip.is_link_local() {
                Some(BlockedBackendReason::LinkLocal)
            } else {
                None
            }
        }
        IpAddr::V6(ip) => {
            if ip.is_loopback() {
                Some(BlockedBackendReason::Localhost)
            } else if (ip.segments()[0] & 0xffc0) == 0xfe80 {
                Some(BlockedBackendReason::LinkLocal)
            } else {
                None
            }
        }
    }
}

/// Check if an address is a localhost/loopback address.
///
/// Kept for backward compatibility; prefer [`classify_backend_addr`] when you also
/// need to gate link-local / metadata ranges.
#[inline]
pub fn is_localhost(addr: &PingoraSocketAddr) -> bool {
    matches!(classify_backend_addr(addr), Some(BlockedBackendReason::Localhost))
}

/// Resolve domain to SocketAddr using async DNS resolution.
///
/// Uses tokio's async DNS resolver to avoid blocking the event loop.
/// Returns the first resolved address.
pub async fn resolve_domain(domain: &str, port: u16) -> pingora_core::Result<SocketAddr> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_localhost_ipv4() {
        // Test IPv4 localhost addresses (127.0.0.0/8)
        let addr = "127.0.0.1:8080".parse::<PingoraSocketAddr>().unwrap();
        assert!(is_localhost(&addr), "127.0.0.1 should be detected as localhost");

        let addr = "127.0.0.100:8080".parse::<PingoraSocketAddr>().unwrap();
        assert!(is_localhost(&addr), "127.0.0.100 should be detected as localhost");

        let addr = "127.255.255.255:8080".parse::<PingoraSocketAddr>().unwrap();
        assert!(is_localhost(&addr), "127.255.255.255 should be detected as localhost");

        // Test non-localhost IPv4 addresses
        let addr = "192.168.1.1:8080".parse::<PingoraSocketAddr>().unwrap();
        assert!(!is_localhost(&addr), "192.168.1.1 should NOT be localhost");

        let addr = "10.0.0.1:8080".parse::<PingoraSocketAddr>().unwrap();
        assert!(!is_localhost(&addr), "10.0.0.1 should NOT be localhost");

        let addr = "8.8.8.8:8080".parse::<PingoraSocketAddr>().unwrap();
        assert!(!is_localhost(&addr), "8.8.8.8 should NOT be localhost");
    }

    #[test]
    fn test_is_localhost_ipv6() {
        // Test IPv6 loopback address
        let addr = "[::1]:8080".parse::<PingoraSocketAddr>().unwrap();
        assert!(is_localhost(&addr), "::1 should be detected as localhost");

        // Test non-localhost IPv6 addresses
        let addr = "[2001:db8::1]:8080".parse::<PingoraSocketAddr>().unwrap();
        assert!(!is_localhost(&addr), "2001:db8::1 should NOT be localhost");

        let addr = "[fe80::1]:8080".parse::<PingoraSocketAddr>().unwrap();
        assert!(!is_localhost(&addr), "fe80::1 should NOT be localhost");
    }

    #[test]
    #[cfg(unix)]
    fn test_is_localhost_unix_socket() {
        use std::os::unix::net::SocketAddr as StdUnixSockAddr;
        use std::path::Path;

        // Unix domain sockets should NOT be considered localhost (they're allowed)
        let path = Path::new("/tmp/test.sock");
        let unix_addr = StdUnixSockAddr::from_pathname(path).unwrap();
        let addr = PingoraSocketAddr::Unix(unix_addr);

        assert!(!is_localhost(&addr), "Unix domain socket should NOT be localhost");
        assert_eq!(
            classify_backend_addr(&addr),
            None,
            "Unix domain socket should be allowed by classify_backend_addr"
        );
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
        let addr = resolve_domain("::1", 9090).await.expect("ipv6 literal resolves");
        assert_eq!(addr.ip().to_string(), "::1");
        assert_eq!(addr.port(), 9090);
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
}
