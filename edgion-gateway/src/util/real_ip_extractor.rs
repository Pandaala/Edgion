//! Real IP Extractor - Extract real client IP from headers with trusted proxy support
//!
//! This module provides functionality to extract the real client IP address from HTTP headers
//! when the gateway is behind trusted proxies (like CDN, load balancers).
//!
//! # Design
//!
//! - **downstream_peer_ip**: The PROXY-effective peer before HTTP header processing
//! - **RemoteIp**: The client IP resolved from trusted forwarded headers
//! - Uses pre-built `IpRadixMatcher` for high-performance CIDR matching
//! - Nginx-style extraction: traverse X-Forwarded-For from right to left, find first non-trusted IP
//!
//! # Example
//!
//! ```text
//! X-Forwarded-For: 203.0.113.1, 198.51.100.2, 192.168.1.1
//! downstream_peer_ip: 192.168.1.254
//! trusted_ips: ["192.168.0.0/16", "198.51.100.0/24"]
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

use edgion_algo::matcher::radix_ip::IpRadixMatcher;
use edgion_resources::matcher::ip_cidr_helpers::to_cidr_format;
use edgion_resources::resources::edgion_config_data::IpGroup;
use pingora_http::RequestHeader;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

/// Real IP extractor with trusted proxy support
pub struct RealIpExtractor {
    /// Pre-built CIDR matcher for trusted proxies
    trusted_proxy_matcher: Option<Arc<IpRadixMatcher>>,

    /// Header name to extract real IP from (e.g., "X-Forwarded-For")
    real_ip_header: String,

    /// Nginx-style `real_ip_recursive` behavior.
    /// - `true`: traverse right-to-left, skip trusted IPs, return first non-trusted.
    /// - `false`: take the rightmost IP from the header directly.
    recursive: bool,

    /// Optional cap on the number of trusted-proxy hops traversed when
    /// `recursive=true`. Mirrors Envoy's `xff_num_trusted_hops`. `None`
    /// means unlimited (nginx-compatible).
    max_trusted_hops: Option<u32>,
}

/// One canonical RealIp resolution result.
///
/// Intentionally does not implement `Debug` so the resolved address cannot be
/// accidentally formatted outside the sensitivity-aware access-log path.
pub(crate) struct RealIpResolution {
    pub(crate) remote_ip: String,
    pub(crate) downstream_trusted: bool,
}

impl RealIpExtractor {
    /// Creates a new RealIpExtractor with trusted proxy IP groups
    ///
    /// # Arguments
    /// * `trusted_ips` - List of `IpGroup` (each with a `name` and `cidrs`)
    /// * `real_ip_header` - Header name to extract real IP from (e.g., "X-Forwarded-For")
    /// * `recursive` - Nginx-style `real_ip_recursive`: true = traverse right-to-left skipping trusted;
    ///   false = take rightmost IP directly
    /// * `max_trusted_hops` - Optional cap on recursive traversal depth (see struct field).
    ///
    /// # Returns
    /// * `Ok(RealIpExtractor)` - Successfully created extractor
    /// * `Err(String)` - Failed to build matcher (only if all CIDRs are invalid and build fails)
    ///
    /// # Notes
    /// - Invalid CIDRs are skipped with a warning log
    /// - If no valid CIDRs are provided, returns extractor with no matcher (all IPs untrusted)
    pub fn new(
        trusted_ips: &[IpGroup],
        real_ip_header: String,
        recursive: bool,
        max_trusted_hops: Option<u32>,
    ) -> Result<Self, String> {
        let mut builder = IpRadixMatcher::builder();
        let mut valid_count: usize = 0;
        let mut invalid_count: usize = 0;

        for group in trusted_ips {
            for cidr in &group.cidrs {
                match to_cidr_format(cidr) {
                    Ok(norm) => match builder.insert(&norm, &group.name) {
                        Ok(_) => valid_count += 1,
                        Err(e) => {
                            invalid_count += 1;
                            tracing::warn!(
                                group = %group.name,
                                cidr = %cidr,
                                error = ?e,
                                "invalid CIDR in trusted_ips, skipping"
                            );
                        }
                    },
                    Err(e) => {
                        invalid_count += 1;
                        tracing::warn!(
                            group = %group.name,
                            cidr = %cidr,
                            error = %e,
                            "invalid CIDR format in trusted_ips, skipping"
                        );
                    }
                }
            }
        }

        // Forgiving semantics: empty/all-invalid → no matcher → every IP
        // returns false from is_trusted_proxy (matches today's behavior).
        let trusted_proxy_matcher = if valid_count == 0 {
            None
        } else {
            Some(Arc::new(
                builder
                    .build()
                    .map_err(|e| format!("Failed to build trusted IP matcher: {}", e))?,
            ))
        };

        if invalid_count > 0 {
            tracing::warn!(
                valid = valid_count,
                invalid = invalid_count,
                "some trusted_ips CIDRs were invalid and skipped"
            );
        }

        Ok(Self {
            trusted_proxy_matcher,
            real_ip_header,
            recursive,
            max_trusted_hops,
        })
    }

    /// Zero-allocation membership check. Hot path; called O(XFF-length)
    /// times per request in `resolve_from_header`.
    pub fn is_trusted_proxy(&self, ip: &IpAddr) -> bool {
        self.trusted_proxy_matcher.as_ref().is_some_and(|m| m.contains(ip))
    }

    /// Returns the matched trusted-proxy group name (longest-prefix), or
    /// `None` when the IP is not trusted or no matcher is configured.
    pub fn matched_trusted_group(&self, ip: &IpAddr) -> Option<&str> {
        self.trusted_proxy_matcher.as_ref().and_then(|m| m.matched_group(ip))
    }

    /// Get the configured real IP header name
    pub fn real_ip_header(&self) -> &str {
        &self.real_ip_header
    }

    /// Extracts the real client IP address using Nginx-style logic
    ///
    /// # Arguments
    /// * `downstream_peer_ip` - PROXY-effective peer IP
    /// * `headers` - Request headers
    ///
    /// # Returns
    /// * Real client IP address as string (without port)
    pub fn extract_real_ip(&self, downstream_peer_ip: &str, headers: &RequestHeader) -> String {
        let header_value = headers
            .headers
            .get(&self.real_ip_header)
            .and_then(|value| value.to_str().ok());
        self.resolve_real_ip(downstream_peer_ip, header_value).remote_ip
    }

    /// Extracts the real client IP from a pre-fetched header value string.
    ///
    /// Used by the RealIp plugin which accesses headers via PluginSession.
    pub fn extract_from_header_value(&self, downstream_peer_ip: &str, header_value: &str) -> String {
        self.resolve_real_ip(downstream_peer_ip, Some(header_value)).remote_ip
    }

    /// Resolve RemoteIp and downstream trust state in one canonical pass.
    ///
    /// The downstream peer is parsed once and checked against the trusted
    /// matcher once. Recursive header traversal still performs one matcher
    /// lookup for each valid forwarded hop.
    pub(crate) fn resolve_real_ip(&self, downstream_peer_ip: &str, header_value: Option<&str>) -> RealIpResolution {
        let downstream_peer_ip = extract_ip_from_socket_addr(downstream_peer_ip);
        let downstream_trusted = self.is_trusted_proxy(&downstream_peer_ip);

        let remote_ip = if downstream_trusted {
            header_value
                .map(|value| self.resolve_from_header(value, &downstream_peer_ip))
                .unwrap_or_else(|| downstream_peer_ip.to_string())
        } else {
            downstream_peer_ip.to_string()
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
    fn resolve_from_header(&self, header_value: &str, downstream_peer_ip: &IpAddr) -> String {
        let iter = || header_value.split(',').map(|s| s.trim()).filter(|s| !s.is_empty());

        if self.recursive {
            let mut leftmost: Option<IpAddr> = None;
            let mut trusted_hops_skipped: u32 = 0;
            // Traverse right-to-left, find first non-trusted IP (or stop at cap)
            for ip_str in iter().rev() {
                if let Ok(ip_addr) = ip_str.parse::<IpAddr>() {
                    leftmost = Some(ip_addr);
                    if !self.is_trusted_proxy(&ip_addr) {
                        return ip_addr.to_string();
                    }
                    // Trusted hop — count it, and stop if we've hit the cap.
                    trusted_hops_skipped += 1;
                    if let Some(cap) = self.max_trusted_hops {
                        if trusted_hops_skipped >= cap {
                            // Caller configured a hop cap; return this (trusted) IP
                            // rather than continuing leftward into potentially-forged territory.
                            return ip_addr.to_string();
                        }
                    }
                }
            }
            // All trusted (no cap hit): return leftmost IP (Nginx behavior)
            leftmost
                .map(|ip| ip.to_string())
                .unwrap_or_else(|| downstream_peer_ip.to_string())
        } else {
            // Non-recursive: take the rightmost IP directly
            let rightmost = iter().rev().find_map(|s| s.parse::<IpAddr>().ok());
            match rightmost {
                Some(ip) if self.is_trusted_proxy(&ip) => downstream_peer_ip.to_string(),
                Some(ip) => ip.to_string(),
                None => downstream_peer_ip.to_string(),
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
