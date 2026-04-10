//! Real IP Extractor - Extract real client IP from headers with trusted proxy support
//!
//! This module provides functionality to extract the real client IP address from HTTP headers
//! when the gateway is behind trusted proxies (like CDN, load balancers).
//!
//! # Design
//!
//! - **client_addr**: The TCP socket address (direct connection)
//! - **remote_addr**: The extracted real client IP (from headers if behind trusted proxy)
//! - Uses pre-built `IpRadixMatcher` for high-performance CIDR matching
//! - Nginx-style extraction: traverse X-Forwarded-For from right to left, find first non-trusted IP
//!
//! # Example
//!
//! ```text
//! X-Forwarded-For: 203.0.113.1, 198.51.100.2, 192.168.1.1
//! client_addr: 192.168.1.254:45678
//! trusted_ips: ["192.168.0.0/16", "198.51.100.0/24"]
//!
//! Logic:
//! 1. client_addr 192.168.1.254 -> matches trusted ✅ continue
//! 2. Traverse XFF right-to-left:
//!    - 192.168.1.1 -> matches trusted ✅ continue
//!    - 198.51.100.2 -> matches trusted ✅ continue
//!    - 203.0.113.1 -> not trusted ❌ this is the real IP
//!
//! Result: remote_addr = "203.0.113.1"
//! ```

use crate::core::common::matcher::ip_radix_tree::{IpRadixError, IpRadixMatcher};
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
}

impl RealIpExtractor {
    /// Creates a new RealIpExtractor with trusted proxy CIDRs
    ///
    /// # Arguments
    /// * `trusted_ips` - List of CIDR strings (e.g., ["10.0.0.0/8", "192.168.0.0/16"])
    /// * `real_ip_header` - Header name to extract real IP from (e.g., "X-Forwarded-For")
    /// * `recursive` - Nginx-style `real_ip_recursive`: true = traverse right-to-left skipping trusted;
    ///   false = take rightmost IP directly
    ///
    /// # Returns
    /// * `Ok(RealIpExtractor)` - Successfully created extractor
    /// * `Err(IpRadixError)` - Failed to build matcher (only if all CIDRs are invalid)
    ///
    /// # Notes
    /// - Invalid CIDRs are skipped with a warning log
    /// - If no valid CIDRs are provided, returns extractor with no matcher (all IPs untrusted)
    pub fn new(trusted_ips: &[String], real_ip_header: String, recursive: bool) -> Result<Self, IpRadixError> {
        let trusted_proxy_matcher = if !trusted_ips.is_empty() {
            let mut builder = IpRadixMatcher::builder();
            let mut valid_count = 0;
            let mut invalid_count = 0;

            for cidr in trusted_ips {
                match builder.insert(cidr, true) {
                    Ok(_) => {
                        valid_count += 1;
                    }
                    Err(e) => {
                        invalid_count += 1;
                        tracing::warn!(
                            cidr = %cidr,
                            error = ?e,
                            "Invalid CIDR in trusted_ips, skipping"
                        );
                    }
                }
            }

            if valid_count > 0 {
                tracing::info!(
                    valid_cidrs = valid_count,
                    invalid_cidrs = invalid_count,
                    "RealIpExtractor CIDR validation completed"
                );
                Some(Arc::new(builder.build()?))
            } else {
                tracing::warn!(
                    total_cidrs = trusted_ips.len(),
                    "No valid CIDRs in trusted_ips, real IP extraction will be disabled"
                );
                None
            }
        } else {
            None
        };

        Ok(Self {
            trusted_proxy_matcher,
            real_ip_header,
            recursive,
        })
    }

    /// Checks if an IP address is a trusted proxy
    ///
    /// # Arguments
    /// * `ip` - IP address to check
    ///
    /// # Returns
    /// * `true` - IP is in trusted proxy list
    /// * `false` - IP is not trusted or no trusted proxies configured
    pub fn is_trusted_proxy(&self, ip: &IpAddr) -> bool {
        self.trusted_proxy_matcher
            .as_ref()
            .and_then(|m| m.match_ip(ip))
            .unwrap_or(false)
    }

    /// Get the configured real IP header name
    pub fn real_ip_header(&self) -> &str {
        &self.real_ip_header
    }

    /// Extracts the real client IP address using Nginx-style logic
    ///
    /// # Arguments
    /// * `client_addr` - TCP client address (socket addr with port)
    /// * `headers` - Request headers
    ///
    /// # Returns
    /// * Real client IP address as string (without port)
    pub fn extract_real_ip(&self, client_addr: &str, headers: &RequestHeader) -> String {
        let client_ip = extract_ip_from_socket_addr(client_addr);

        if self.trusted_proxy_matcher.is_none() || !self.is_trusted_proxy(&client_ip) {
            return client_ip.to_string();
        }

        if let Some(header_value) = headers.headers.get(&self.real_ip_header) {
            if let Ok(xff_str) = header_value.to_str() {
                return self.resolve_from_header(xff_str, &client_ip);
            }
        }

        client_ip.to_string()
    }

    /// Extracts the real client IP from a pre-fetched header value string.
    ///
    /// Used by the RealIp plugin which accesses headers via PluginSession.
    pub fn extract_from_header_value(&self, client_addr: &str, header_value: &str) -> String {
        let client_ip = extract_ip_from_socket_addr(client_addr);

        if self.trusted_proxy_matcher.is_none() || !self.is_trusted_proxy(&client_ip) {
            return client_ip.to_string();
        }

        self.resolve_from_header(header_value, &client_ip)
    }

    /// Core extraction logic shared by both entry points.
    ///
    /// Nginx behavior:
    /// - `recursive = true`: traverse right-to-left, skip trusted IPs, return first non-trusted.
    /// - `recursive = false`: take rightmost IP; if trusted, use client_addr; if not, use it.
    /// - Fallback when all IPs trusted (recursive mode): return leftmost IP.
    fn resolve_from_header(&self, header_value: &str, client_ip: &IpAddr) -> String {
        let iter = || {
            header_value
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
        };

        if self.recursive {
            let mut leftmost: Option<IpAddr> = None;
            // Traverse right-to-left, find first non-trusted IP
            for ip_str in iter().rev() {
                if let Ok(ip_addr) = ip_str.parse::<IpAddr>() {
                    leftmost = Some(ip_addr);
                    if !self.is_trusted_proxy(&ip_addr) {
                        return ip_addr.to_string();
                    }
                }
            }
            // All trusted: return leftmost IP (Nginx behavior)
            leftmost
                .map(|ip| ip.to_string())
                .unwrap_or_else(|| client_ip.to_string())
        } else {
            // Non-recursive: take the rightmost IP directly
            let rightmost = iter()
                .rev()
                .find_map(|s| s.parse::<IpAddr>().ok());
            match rightmost {
                Some(ip) if self.is_trusted_proxy(&ip) => client_ip.to_string(),
                Some(ip) => ip.to_string(),
                None => client_ip.to_string(),
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

/// Extracts IP address from socket address string and returns as string (without port)
///
/// # Arguments
/// * `addr` - Socket address string (e.g., "192.168.1.1:45678")
///
/// # Returns
/// * IP address as string (e.g., "192.168.1.1")
pub fn extract_ip_string(addr: &str) -> String {
    extract_ip_from_socket_addr(addr).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_trusted_ips() {
        let extractor = RealIpExtractor::new(&[], "X-Forwarded-For".to_string(), true).unwrap();

        let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
        headers.insert_header("X-Forwarded-For", "203.0.113.1").unwrap();

        let result = extractor.extract_real_ip("192.168.1.1:45678", &headers);
        assert_eq!(result, "192.168.1.1");
    }

    #[test]
    fn test_client_not_trusted() {
        let extractor =
            RealIpExtractor::new(&["10.0.0.0/8".to_string()], "X-Forwarded-For".to_string(), true).unwrap();

        let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
        headers.insert_header("X-Forwarded-For", "203.0.113.1").unwrap();

        // Client 192.168.1.1 is not in 10.0.0.0/8
        let result = extractor.extract_real_ip("192.168.1.1:45678", &headers);
        assert_eq!(result, "192.168.1.1");
    }

    #[test]
    fn test_recursive_extraction() {
        let extractor = RealIpExtractor::new(
            &["192.168.0.0/16".to_string(), "198.51.100.0/24".to_string()],
            "X-Forwarded-For".to_string(),
            true,
        )
        .unwrap();

        let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
        headers
            .insert_header("X-Forwarded-For", "203.0.113.1, 198.51.100.2, 192.168.1.1")
            .unwrap();

        // Recursive: 192.168.1.1 (trusted) -> 198.51.100.2 (trusted) -> 203.0.113.1 (NOT trusted)
        let result = extractor.extract_real_ip("192.168.1.254:45678", &headers);
        assert_eq!(result, "203.0.113.1");
    }

    #[test]
    fn test_non_recursive_extraction() {
        let extractor = RealIpExtractor::new(
            &["192.168.0.0/16".to_string(), "198.51.100.0/24".to_string()],
            "X-Forwarded-For".to_string(),
            false,
        )
        .unwrap();

        // Non-recursive: rightmost 192.168.1.1 is trusted → use client_addr
        let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
        headers
            .insert_header("X-Forwarded-For", "203.0.113.1, 198.51.100.2, 192.168.1.1")
            .unwrap();
        let result = extractor.extract_real_ip("192.168.1.254:45678", &headers);
        assert_eq!(result, "192.168.1.254");

        // Non-recursive: rightmost 8.8.8.8 is NOT trusted → use it
        let mut headers2 = RequestHeader::build("GET", b"/", None).unwrap();
        headers2
            .insert_header("X-Forwarded-For", "203.0.113.1, 8.8.8.8")
            .unwrap();
        let result2 = extractor.extract_real_ip("192.168.1.254:45678", &headers2);
        assert_eq!(result2, "8.8.8.8");
    }

    #[test]
    fn test_all_ips_trusted_fallback() {
        let extractor = RealIpExtractor::new(
            &["0.0.0.0/0".to_string()], // All IPs trusted
            "X-Forwarded-For".to_string(),
            true,
        )
        .unwrap();

        let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
        headers
            .insert_header("X-Forwarded-For", "203.0.113.1, 198.51.100.2")
            .unwrap();

        // All IPs trusted, recursive: return leftmost IP (Nginx behavior)
        let result = extractor.extract_real_ip("192.168.1.254:45678", &headers);
        assert_eq!(result, "203.0.113.1");
    }

    #[test]
    fn test_extract_ip_string() {
        assert_eq!(extract_ip_string("192.168.1.1:45678"), "192.168.1.1");
        assert_eq!(extract_ip_string("192.168.1.1"), "192.168.1.1");
        assert_eq!(extract_ip_string("[::1]:8080"), "::1");
    }

    #[test]
    fn test_invalid_cidrs_skipped() {
        let extractor = RealIpExtractor::new(
            &[
                "10.0.0.0/8".to_string(),
                "invalid-cidr".to_string(),
                "192.168.0.0/16".to_string(),
                "300.0.0.0/8".to_string(),
                "172.16.0.0/12".to_string(),
            ],
            "X-Forwarded-For".to_string(),
            true,
        )
        .unwrap();

        let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
        headers
            .insert_header("X-Forwarded-For", "203.0.113.1, 10.0.0.1")
            .unwrap();

        let result = extractor.extract_real_ip("192.168.1.254:45678", &headers);
        assert_eq!(result, "203.0.113.1");
    }

    #[test]
    fn test_all_invalid_cidrs() {
        let extractor = RealIpExtractor::new(
            &["invalid-cidr".to_string(), "not-an-ip/24".to_string()],
            "X-Forwarded-For".to_string(),
            true,
        )
        .unwrap();

        let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
        headers.insert_header("X-Forwarded-For", "203.0.113.1").unwrap();

        let result = extractor.extract_real_ip("192.168.1.1:45678", &headers);
        assert_eq!(result, "192.168.1.1");
    }
}
