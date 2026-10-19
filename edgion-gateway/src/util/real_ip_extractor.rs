//! Real IP Extractor - Extract real client IP from headers with trusted proxy support
//!
//! This module provides functionality to extract the real client IP address from HTTP headers
//! when the gateway is behind trusted proxies (like CDN, load balancers).
//!
//! # Design
//!
//! - **downstream_peer_ip**: The PROXY-effective peer before HTTP header processing
//! - **RemoteIp**: The client IP resolved from trusted forwarded headers
//! - Uses the caller's captured canonical IpList membership view
//! - Nginx-style extraction: traverse X-Forwarded-For from right to left, find first non-trusted IP
//!
//! # Example
//!
//! ```text
//! X-Forwarded-For: 203.0.113.1, 198.51.100.2, 192.168.1.1
//! downstream_peer_ip: 192.168.1.254
//! trusted IpList entries: ["192.168.0.0/16", "198.51.100.0/24"]
//!
//! Logic:
//! 1. downstream peer 192.168.1.254 -> matches trusted ✅ continue
//! 2. Traverse XFF right-to-left:
//!    - 192.168.1.1 -> matches trusted ✅ continue
//!    - 198.51.100.2 -> matches trusted ✅ continue
//!    - 203.0.113.1 -> not trusted ❌ this is the real IP
//!
//! Result: RemoteIp = "203.0.113.1"
//! ```

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// Header traversal policy; the caller supplies one captured trust decision view.
pub struct RealIpExtractor {
    real_ip_header: String,
    recursive: bool,
    max_trusted_hops: Option<u32>,
}

/// One canonical RealIp resolution result.
///
/// Intentionally does not implement `Debug` so the resolved address cannot be
/// accidentally formatted outside the sensitivity-aware access-log path.
pub(crate) struct RealIpResolution {
    /// Preserve the selected address family; consumers own textual normalization.
    pub(crate) remote_ip: IpAddr,
    pub(crate) downstream_trusted: bool,
}

impl RealIpExtractor {
    pub(crate) fn new(real_ip_header: String, recursive: bool, max_trusted_hops: Option<u32>) -> Self {
        Self {
            real_ip_header,
            recursive,
            max_trusted_hops,
        }
    }
    pub(crate) fn real_ip_header(&self) -> &str {
        &self.real_ip_header
    }

    /// Resolve RemoteIp and downstream trust state in one canonical pass.
    ///
    /// The downstream peer is parsed once and checked against the trusted
    /// matcher once. Recursive header traversal still performs one matcher
    /// lookup for each valid forwarded hop.
    pub(crate) fn resolve_real_ip(
        &self,
        downstream_peer_ip: &str,
        header_value: Option<&str>,
        mut is_trusted: impl FnMut(&IpAddr) -> bool,
    ) -> RealIpResolution {
        let downstream_peer_ip = extract_ip_from_socket_addr(downstream_peer_ip);
        let downstream_trusted = is_trusted(&downstream_peer_ip);

        let remote_ip = if downstream_trusted {
            header_value
                .map(|value| self.resolve_from_header(value, &downstream_peer_ip, &mut is_trusted))
                .unwrap_or(downstream_peer_ip)
        } else {
            downstream_peer_ip
        };

        RealIpResolution {
            remote_ip,
            downstream_trusted,
        }
    }

    /// Core extraction logic shared by both entry points.
    ///
    /// Nginx behavior:
    /// - `recursive = true`: traverse right-to-left, skip trusted IPs, return first non-trusted.
    /// - `recursive = false`: take rightmost IP; if trusted, use the downstream peer; if not, use it.
    /// - Fallback when all IPs trusted (recursive mode): return leftmost IP.
    ///
    /// When `max_trusted_hops` is `Some(N)`, the right-to-left traversal stops
    /// after skipping `N` trusted hops — caps the attacker's ability to push
    /// the result leftward by forging a long trusted-looking prefix.
    fn resolve_from_header(
        &self,
        header_value: &str,
        downstream_peer_ip: &IpAddr,
        is_trusted: &mut impl FnMut(&IpAddr) -> bool,
    ) -> IpAddr {
        let iter = || header_value.split(',').map(|s| s.trim()).filter(|s| !s.is_empty());

        if self.recursive {
            let mut leftmost: Option<IpAddr> = None;
            let mut trusted_hops_skipped: u32 = 0;
            // Traverse right-to-left, find first non-trusted IP (or stop at cap)
            for ip_str in iter().rev() {
                if let Ok(ip_addr) = ip_str.parse::<IpAddr>() {
                    leftmost = Some(ip_addr);
                    if !is_trusted(&ip_addr) {
                        return ip_addr;
                    }
                    // Trusted hop — count it, and stop if we've hit the cap.
                    trusted_hops_skipped += 1;
                    if let Some(cap) = self.max_trusted_hops {
                        if trusted_hops_skipped >= cap {
                            // Caller configured a hop cap; return this (trusted) IP
                            // rather than continuing leftward into potentially-forged territory.
                            return ip_addr;
                        }
                    }
                }
            }
            // All trusted (no cap hit): return leftmost IP (Nginx behavior)
            leftmost.unwrap_or(*downstream_peer_ip)
        } else {
            // Non-recursive: take the rightmost IP directly
            let rightmost = iter().rev().find_map(|s| s.parse::<IpAddr>().ok());
            match rightmost {
                Some(ip) if is_trusted(&ip) => *downstream_peer_ip,
                Some(ip) => ip,
                None => *downstream_peer_ip,
            }
        }
    }
}

/// Extracts IP address from socket address string (removes port)
///
/// # Arguments
/// * `addr` - Socket address string (e.g., "192.168.1.1:45678" or "192.168.1.1")
///
/// # Returns
/// * IP address (0.0.0.0 if parsing fails)
fn extract_ip_from_socket_addr(addr: &str) -> IpAddr {
    addr.parse::<SocketAddr>()
        .map(|sa| sa.ip())
        .or_else(|_| addr.parse::<IpAddr>())
        .unwrap_or_else(|_| IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)))
}

#[cfg(test)]
#[path = "../../tests/unit/util/real_ip_extractor_test.rs"]
mod tests;
