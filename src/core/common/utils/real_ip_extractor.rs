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
}

impl RealIpExtractor {
    /// Creates a new RealIpExtractor with trusted proxy CIDRs
    ///
    /// # Arguments
    /// * `trusted_ips` - List of CIDR strings (e.g., ["10.0.0.0/8", "192.168.0.0/16"])
    /// * `real_ip_header` - Header name to extract real IP from (e.g., "X-Forwarded-For")
    ///
    /// # Returns
    /// * `Ok(RealIpExtractor)` - Successfully created extractor
    /// * `Err(IpRadixError)` - Failed to build matcher (only if all CIDRs are invalid)
    ///
    /// # Notes
    /// - Invalid CIDRs are skipped with a warning log
    /// - If no valid CIDRs are provided, returns extractor with no matcher (all IPs untrusted)
    pub fn new(trusted_ips: &[String], real_ip_header: String) -> Result<Self, IpRadixError> {
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
    /// # Algorithm
    /// 1. If no trusted proxies configured, return client_addr as-is
    /// 2. If client_addr is not a trusted proxy, return client_addr
    /// 3. Extract IPs from the configured header (e.g., X-Forwarded-For)
    /// 4. Traverse from right to left, find first non-trusted IP
    /// 5. Fallback to client_addr if no valid IP found
    ///
    /// # Arguments
    /// * `client_addr` - TCP client address (socket addr with port)
    /// * `headers` - Request headers
    ///
    /// # Returns
    /// * Real client IP address as string (without port)
    pub fn extract_real_ip(&self, client_addr: &str, headers: &RequestHeader) -> String {
        // 1. If no trusted proxies configured, return client_addr
        if self.trusted_proxy_matcher.is_none() {
            return extract_ip_from_socket_addr(client_addr).to_string();
        }

        // 2. Check if client_addr is a trusted proxy
        let client_ip = extract_ip_from_socket_addr(client_addr);
        if !self.is_trusted_proxy(&client_ip) {
            return client_ip.to_string();
        }

        // 3. Extract IPs from the configured header (Nginx-style: right-to-left)
        if let Some(header_value) = headers.headers.get(&self.real_ip_header) {
            if let Ok(xff_str) = header_value.to_str() {
                // Parse comma-separated IP list
                let ips: Vec<&str> = xff_str.split(',').map(|s| s.trim()).collect();

                // Traverse from right to left
                for ip_str in ips.iter().rev() {
                    if let Ok(ip_addr) = ip_str.parse::<IpAddr>() {
                        // Find first non-trusted IP
                        if !self.is_trusted_proxy(&ip_addr) {
                            return ip_addr.to_string();
                        }
                    }
                }
            }
        }

        // 4. Fallback to client_addr
        client_ip.to_string()
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
        let extractor = RealIpExtractor::new(&[], "X-Forwarded-For".to_string()).unwrap();

        let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
        headers.insert_header("X-Forwarded-For", "203.0.113.1").unwrap();

        let result = extractor.extract_real_ip("192.168.1.1:45678", &headers);
        assert_eq!(result, "192.168.1.1");
    }

    #[test]
    fn test_client_not_trusted() {
        let extractor = RealIpExtractor::new(&["10.0.0.0/8".to_string()], "X-Forwarded-For".to_string()).unwrap();

        let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
        headers.insert_header("X-Forwarded-For", "203.0.113.1").unwrap();

        // Client 192.168.1.1 is not in 10.0.0.0/8
        let result = extractor.extract_real_ip("192.168.1.1:45678", &headers);
        assert_eq!(result, "192.168.1.1");
    }

    #[test]
    fn test_nginx_style_extraction() {
        let extractor = RealIpExtractor::new(
            &["192.168.0.0/16".to_string(), "198.51.100.0/24".to_string()],
            "X-Forwarded-For".to_string(),
        )
        .unwrap();

        let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
        headers
            .insert_header("X-Forwarded-For", "203.0.113.1, 198.51.100.2, 192.168.1.1")
            .unwrap();

        // Client is 192.168.1.254 (trusted)
        // XFF traversal: 192.168.1.1 (trusted) -> 198.51.100.2 (trusted) -> 203.0.113.1 (NOT trusted)
        let result = extractor.extract_real_ip("192.168.1.254:45678", &headers);
        assert_eq!(result, "203.0.113.1");
    }

    #[test]
    fn test_all_ips_trusted_fallback() {
        let extractor = RealIpExtractor::new(
            &["0.0.0.0/0".to_string()], // All IPs trusted
            "X-Forwarded-For".to_string(),
        )
        .unwrap();

        let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
        headers
            .insert_header("X-Forwarded-For", "203.0.113.1, 198.51.100.2")
            .unwrap();

        // All IPs are trusted, fallback to client_addr
        let result = extractor.extract_real_ip("192.168.1.254:45678", &headers);
        assert_eq!(result, "192.168.1.254");
    }

    #[test]
    fn test_extract_ip_string() {
        assert_eq!(extract_ip_string("192.168.1.1:45678"), "192.168.1.1");
        assert_eq!(extract_ip_string("192.168.1.1"), "192.168.1.1");
        assert_eq!(extract_ip_string("[::1]:8080"), "::1");
    }

    #[test]
    fn test_invalid_cidrs_skipped() {
        // Mix of valid and invalid CIDRs
        let extractor = RealIpExtractor::new(
            &[
                "10.0.0.0/8".to_string(),     // valid
                "invalid-cidr".to_string(),   // invalid
                "192.168.0.0/16".to_string(), // valid
                "300.0.0.0/8".to_string(),    // invalid IP
                "172.16.0.0/12".to_string(),  // valid
            ],
            "X-Forwarded-For".to_string(),
        )
        .unwrap();

        // Should have valid CIDRs working
        let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
        headers
            .insert_header("X-Forwarded-For", "203.0.113.1, 10.0.0.1")
            .unwrap();

        // Client 192.168.1.1 is trusted (valid CIDR), should extract from XFF
        let result = extractor.extract_real_ip("192.168.1.254:45678", &headers);
        assert_eq!(result, "203.0.113.1");
    }

    #[test]
    fn test_all_invalid_cidrs() {
        // All CIDRs are invalid - extractor should work but with no trusted proxies
        let extractor = RealIpExtractor::new(
            &["invalid-cidr".to_string(), "not-an-ip/24".to_string()],
            "X-Forwarded-For".to_string(),
        )
        .unwrap();

        let mut headers = RequestHeader::build("GET", b"/", None).unwrap();
        headers.insert_header("X-Forwarded-For", "203.0.113.1").unwrap();

        // No valid CIDRs, so should just return client_addr
        let result = extractor.extract_real_ip("192.168.1.1:45678", &headers);
        assert_eq!(result, "192.168.1.1");
    }
}
