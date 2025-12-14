use anyhow::{Context, Result};
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use pingora_core::protocols::l4::socket::SocketAddr as PingoraSocketAddr;

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

/// Check if an address is a localhost/loopback address
/// 
/// Returns true if the address is:
/// - 127.0.0.0/8 (IPv4 loopback range)
/// - ::1 (IPv6 loopback)
/// - Unix domain sockets are allowed (return false)
#[inline]
pub fn is_localhost(addr: &PingoraSocketAddr) -> bool {
    // Only check Inet sockets, Unix domain sockets are allowed
    if let Some(inet_addr) = addr.as_inet() {
        match inet_addr.ip() {
            IpAddr::V4(ip) => {
                // Check if IP is in 127.0.0.0/8 range
                ip.octets()[0] == 127
            }
            IpAddr::V6(ip) => {
                // Check if IP is ::1 (IPv6 loopback)
                ip.is_loopback()
            }
        }
    } else {
        // Unix domain sockets are allowed
        false
    }
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
    }
}
